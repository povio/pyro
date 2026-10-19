# Percentage widths with definite parents

The office badge component measures a full-width container inside a 320px table cell before deciding how many labels fit. Previously the CSS dimension parser discarded percentages as `auto`, so that container reported the synthetic 5px fallback. The original label assertions are valid; replacing them with a `+2` expectation would conceal this native measurement failure.

The native dimension value now retains percentages. Layout resolves a width through a chain of percentage-width parents until it reaches a definite declared width. Resolution walks parents iteratively, without deriving a basis from child content or recursing through layout. Existing style observation/invalidation applies to parent size changes, reparenting, stylesheet declarations and custom properties. Root boxes retain their inline-only dimension policy.

Automatic or intrinsic parent widths, positioned containing blocks, percentage heights, full CSS box sizing and intrinsic text/flex/gap measurements remain outside this bounded repair. Those cases retain their previous fallback; this is not full CSS layout or font measurement. No application labels, test selectors or badge-specific dimensions are introduced.
