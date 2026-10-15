//! A small, bounded walk over the XMP CorelDRAW writes: leaf elements become scalar fields,
//! `rdf:Bag`/`rdf:Seq`/`rdf:Alt` items become lists, and `TextData` becomes text runs.
//!
//! Paths are the local names below `rdf:Description`, joined with `/`, skipping the RDF container
//! and item elements: `<Objects><Total>17</Total>` is `Objects/Total`, and the items of
//! `<FontsUsed><rdf:Bag><rdf:li>` are the list `FontsUsed`. Namespace prefixes are dropped because
//! CorelDRAW puts its fields in a default namespace.

use std::collections::BTreeMap;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::{Error, Limits};

#[derive(Debug, Default)]
pub(crate) struct Xmp {
    pub scalars: BTreeMap<String, String>,
    pub lists: BTreeMap<String, Vec<String>>,
    /// Attributes of empty elements such as `<ClipRect left=… />`, by path.
    pub attrs: BTreeMap<String, Vec<(String, String)>>,
    pub text_streams: Vec<RawStream>,
}

#[derive(Debug, Default)]
pub(crate) struct RawStream {
    pub runs: Vec<RawRun>,
}

#[derive(Debug, Default)]
pub(crate) struct RawRun {
    pub text: String,
    pub attrs: Vec<(String, String)>,
}

struct Frame {
    name: String,
    text: String,
    has_child: bool,
    attrs: Vec<(String, String)>,
}

fn is_container(name: &str) -> bool {
    matches!(name, "Bag" | "Seq" | "Alt" | "li")
}

struct Walker<'l> {
    entry: &'l str,
    limits: &'l Limits,
    stack: Vec<Frame>,
    out: Xmp,
    items: usize,
}

impl Walker<'_> {
    fn err(&self, message: impl ToString) -> Error {
        Error::Xml { entry: self.entry.to_owned(), message: message.to_string() }
    }

    fn count(&mut self) -> Result<(), Error> {
        self.items += 1;
        if self.items > self.limits.max_items {
            return Err(Error::Limit { what: "XML items", limit: self.limits.max_items });
        }
        Ok(())
    }

    /// Path of the frames below `rdf:Description`, or `None` outside one.
    fn path(&self) -> Option<String> {
        let start = self.stack.iter().rposition(|f| f.name == "Description")?;
        let parts: Vec<&str> = self.stack.iter().skip(start + 1).map(|f| f.name.as_str()).filter(|n| !is_container(n)).collect();
        Some(parts.join("/"))
    }

    fn attributes(&self, e: &BytesStart<'_>) -> Result<Vec<(String, String)>, Error> {
        let mut out = Vec::new();
        for a in e.attributes() {
            let a = a.map_err(|x| self.err(x))?;
            let key = String::from_utf8_lossy(a.key.local_name().as_ref()).into_owned();
            let value = a.normalized_value(quick_xml::XmlVersion::Implicit1_0).map_err(|x| self.err(x))?.into_owned();
            out.push((key, value));
        }
        Ok(out)
    }

    fn start(&mut self, e: &BytesStart<'_>) -> Result<(), Error> {
        if self.stack.len() >= self.limits.max_depth {
            return Err(Error::Limit { what: "nested XML elements", limit: self.limits.max_depth });
        }
        let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
        if let Some(parent) = self.stack.last_mut() {
            parent.has_child = true;
        }
        let in_text_data = self.stack.iter().any(|f| f.name == "TextData");
        let attrs = if name == "TextRun" || !in_text_data { self.attributes(e)? } else { Vec::new() };
        if in_text_data && name == "TextStream" {
            self.count()?;
            self.out.text_streams.push(RawStream::default());
        }
        self.stack.push(Frame { name, text: String::new(), has_child: false, attrs });
        // A list that is present but empty (`<rdf:Bag />` or `<rdf:Bag></rdf:Bag>`) still exists.
        if matches!(self.stack.last().map(|f| f.name.as_str()), Some("Bag" | "Seq" | "Alt"))
            && let Some(path) = self.path()
        {
            self.out.lists.entry(path).or_default();
        }
        Ok(())
    }

    fn end(&mut self) -> Result<(), Error> {
        let path = self.path();
        let frame = self.stack.pop().ok_or_else(|| self.err("closing tag without an opening tag"))?;
        let in_text_data = self.stack.iter().any(|f| f.name == "TextData");
        if frame.name == "TextRun" && in_text_data {
            self.count()?;
            if self.out.text_streams.is_empty() {
                self.out.text_streams.push(RawStream::default());
            }
            if let Some(stream) = self.out.text_streams.last_mut() {
                stream.runs.push(RawRun { text: frame.text, attrs: frame.attrs });
            }
            return Ok(());
        }
        let Some(path) = path else { return Ok(()) };
        if in_text_data || frame.name == "TextData" {
            return Ok(());
        }
        if frame.name == "li" {
            self.count()?;
            self.out.lists.entry(path).or_default().push(frame.text.trim().to_owned());
        } else if !frame.has_child && !is_container(&frame.name) && !path.is_empty() {
            self.count()?;
            if !frame.attrs.is_empty() {
                self.out.attrs.insert(path.clone(), frame.attrs);
            }
            self.out.scalars.insert(path, frame.text.trim().to_owned());
        }
        Ok(())
    }

    fn text(&mut self, s: &str) -> Result<(), Error> {
        let Some(top) = self.stack.last_mut() else { return Ok(()) };
        if top.has_child && s.trim().is_empty() {
            return Ok(());
        }
        // Text is bounded by the entry size the package already capped.
        top.text.push_str(s);
        Ok(())
    }
}

pub(crate) fn parse(xml: &str, entry: &str, limits: &Limits) -> Result<Xmp, Error> {
    let mut reader = Reader::from_str(xml);
    let mut w = Walker { entry, limits, stack: Vec::new(), out: Xmp::default(), items: 0 };
    loop {
        let event = reader.read_event().map_err(|e| w.err(format!("{e} at byte {}", reader.error_position())))?;
        match event {
            Event::Start(e) => w.start(&e)?,
            Event::Empty(e) => {
                w.start(&e)?;
                w.end()?;
            }
            Event::End(_) => w.end()?,
            Event::Text(t) => {
                let s = t.xml10_content().map_err(|e| w.err(e))?;
                w.text(&s)?;
            }
            Event::CData(t) => {
                let s = t.decode().map_err(|e| w.err(e))?;
                w.text(&s)?;
            }
            Event::GeneralRef(r) => {
                let resolved = match r.resolve_char_ref().map_err(|e| w.err(e))? {
                    Some(c) => c.to_string(),
                    None => {
                        let name = r.decode().map_err(|e| w.err(e))?;
                        quick_xml::escape::resolve_predefined_entity(&name).ok_or_else(|| w.err(format!("unknown entity &{name};")))?.to_owned()
                    }
                };
                w.text(&resolved)?;
            }
            Event::Eof => break,
            Event::Comment(_) | Event::Decl(_) | Event::PI(_) | Event::DocType(_) => {}
        }
    }
    if !w.stack.is_empty() {
        return Err(w.err("document ends inside an element"));
    }
    Ok(w.out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn walk(xml: &str) -> Result<Xmp, Error> {
        parse(xml, "t.xml", &Limits::default())
    }

    #[test]
    fn paths_skip_rdf_containers_and_prefixes() {
        let x = walk(
            r#"<x:m><rdf:RDF><rdf:Description><A><B>1</B><C:D> 2 </C:D></A>
               <L><rdf:Seq><rdf:li>p</rdf:li><rdf:li>&#x71;</rdf:li></rdf:Seq></L><E/><F><rdf:Bag/></F>
               <R l="1" t="&lt;2"/></rdf:Description></rdf:RDF></x:m>"#,
        )
        .unwrap();
        assert_eq!(x.scalars.get("A/B").map(String::as_str), Some("1"));
        assert_eq!(x.scalars.get("A/D").map(String::as_str), Some("2"));
        assert_eq!(x.scalars.get("E").map(String::as_str), Some(""));
        assert_eq!(x.lists.get("L").unwrap(), &["p", "q"]);
        assert!(x.lists.get("F").unwrap().is_empty());
        assert_eq!(x.attrs.get("R").unwrap(), &[("l".to_owned(), "1".to_owned()), ("t".to_owned(), "<2".to_owned())]);
    }

    #[test]
    fn ignores_fields_outside_description() {
        let x = walk("<x:m><Stray>1</Stray></x:m>").unwrap();
        assert!(x.scalars.is_empty());
    }

    #[test]
    fn text_runs_keep_whitespace() {
        let x =
            walk(r#"<rdf:Description><TextData><TextStream><TextRun break="para"> a </TextRun></TextStream></TextData></rdf:Description>"#).unwrap();
        assert_eq!(x.text_streams[0].runs[0].text, " a ");
        assert!(x.scalars.is_empty());
    }

    #[test]
    fn depth_unknown_entities_and_truncation_fail() {
        let deep = "<a>".repeat(100);
        assert!(matches!(walk(&deep), Err(Error::Limit { .. })));
        assert!(matches!(walk("<rdf:Description><A>&nope;</A></rdf:Description>"), Err(Error::Xml { .. })));
        assert!(matches!(walk("<rdf:Description><A>"), Err(Error::Xml { .. })));
    }
}
