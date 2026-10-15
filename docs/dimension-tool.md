# Dimension annotations

> **Last updated:** 2026-10-15 · **Status:** initial implementation; measurements are snapshots, not constraints on the source artwork.

Choose **Dimension Tool** in the Modify group of All Tools (or the tool search). Its floating tool bar always exposes Linear, Angular, Radius and Diameter, Rectangle Width/Height modes and the options dialog, including in Essentials with the optional Control bar hidden. The initial mode is Linear; the selected mode remains visibly highlighted. Double-clicking the tool also opens its options. The optional Control bar also offers one-click Width/Height annotations for selected artwork.

## Place a dimension

- Linear: drag outward from a straight path edge. Alternatively click two points, then click where the dimension line should sit. Dragging in empty space picks the two endpoints; the next click places the label. Shift constrains the second point to 45-degree increments.
- Rectangle Width/Height: choose Width or Height in the floating bar, then drag outward from any edge of a rectangle. The annotation measures the complete horizontal or vertical bounds, rather than only the picked edge. These modes also work on other path bounds; manually picked points are projected onto the chosen axis.
- Angular: drag outward from a path corner, or click the vertex, a point on each ray, then the label position. The label position selects the interior or reflex angle.
- Radius/diameter: drag outward from the circumference of a live circle or circular arc, or pick the centre, a point on the rim, then the label position. Arbitrary curved paths are not automatically fitted to circles.
- Escape cancels unfinished point picking. Backspace removes the last picked point. Switching tools cancels unfinished annotations.

Annotations are grouped vector paths and text. They remain visible when switching tools, print and export with the drawing, and can be selected and moved as a group. Each placement is one Undo step.

## Separate annotation layer

Placing the first annotation automatically creates a top-level **Dimensions** layer. Subsequent annotations reuse the dedicated layer, even after it is renamed. A normal artwork layer named Dimensions is not reused. If the annotation layer is hidden, locked or a template, a new visible, editable annotation layer is created without altering the existing layer. Creating the layer and placing the first annotation undo together. The source artwork and its layer are not changed.

## Change appearance

The options dialog includes measurement units, 0–6 decimal places shown as format examples (`0`, `0.0`, `0.00`, `0.000`, etc.), hidden unit suffixes and drawing scale (1:n means one drawing unit represents n real units). Scale affects lengths, not angles. Dimension lines and extension lines have independent weights, solid/dashed/dotted styles and colours. Arrowheads can be closed, open, ticks or absent and have their own scale. Extension lines have a gap, overrun and hide switch.

The compact options window uses four bordered groups, paired fields, stroke and arrow previews, and Reset/Save/Cancel actions. Five text-placement buttons choose outside/inside (relative to the measured object) and start/centre/end alignment. Right-click a placement button for the additional On Line option. Text settings include installed font family/style menus, text size with a display-unit menu, colour and offset. Length fields accept units such as `5 mm` as well as points. Generic font defaults resolve to a concrete available face in generated text, avoiding a spurious missing-font highlight. Available glyphs depend on the installed fonts; no proprietary fonts are bundled.

With a dimension group selected, **Properties > Dimensions > Dimension Options…** changes that annotation in one Undo step. With no dimension selected, the tool dialog changes defaults for future annotations; these persist across tool switches and application preference saves. Cancel does not apply settings. Reset restores the draft defaults until confirmed.

## Files and limits

The native `.vectorcraft` file stores the editable recipe and the dedicated-layer marker. SVG/PDF exports retain ordinary vector artwork; importing an exported file does not promise preservation of the editable recipe. Source geometry is measured at placement time: editing or moving the original shape does not automatically remeasure an existing annotation. The tool does not change the dimensions of the source shape, and is not a CAD constraint system. Automatic radial picking requires a live circle/circular arc, not an arbitrary ellipse. Small or crowded annotations may need manual text sizing/placement.

Commands: `dimension.create`, `dimension.fromSelection`, `dimension.inspect` and `dimension.update`. Tool defaults use the existing `tool.setOption` command with `{tool: "dimension", values: {kind, axis, style}}`; `axis` is `aligned`, `horizontal` or `vertical` for the linear modes. The stored `textPosition` names `above` and `below` mean outside and inside relative to the object; `inline` is the additional on-line placement.

## Public behaviour references

Implemented independently from public documentation and contributor-provided screenshots; no Adobe application, code, icons, presets or fonts were used.

- [Dimension tool overview](https://helpx.adobe.com/illustrator/using/tool-techniques/dimension-tool.html)
- [Dimension tool options](https://helpx.adobe.com/illustrator/desktop/measure-and-align/plot-and-measure/dimension-tool-options-and-settings.html)

## Revision history

| Date | Change |
|---|---|
| 2026-10-15 | Initial dimension tool, persistent vector annotations, dedicated layer, style dialog and command/UI regressions. |
