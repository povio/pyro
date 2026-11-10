# Editing, clipboard and caret APIs

Code: `crates/lp-webapi/src/editing.rs` (selection, contenteditable typing),
`edit_commands.rs` (granular moves and deletions, `Selection.modify`,
multi-click, `execCommand`/`queryCommand*`, undo), `clipboard.rs` (copy, cut,
paste) and `editing_globals.rs`/`.js` (`ClipboardEvent`, `CaretPosition`,
`caretRangeFromPoint`, `caretPositionFromPoint`). Behaviour follows headless
Chromium.

## Keys

The platform is `navigator.platform` (the host's unless overridden): macOS
moves by word with Alt, to the line's ends with Meta+Left/Right and to the
document's with Meta+Up/Down; elsewhere Ctrl moves by word and Ctrl+Home/End
goes to the document's ends. The shortcut modifier for copy (C), cut (X),
paste (V; with Shift: text only), undo (Z) and redo (Shift+Z, Ctrl+Y off
macOS) is Meta on macOS and Ctrl elsewhere. Ctrl/Meta key presses fire no
`keypress` and insert no text.

| Keys | Text controls | contenteditable |
|---|---|---|
| word + Left/Right (Shift extends) | word start/end | same, across nodes |
| word + Backspace/Delete | `deleteWordBackward/Forward` | same |
| Meta+Backspace (macOS) | `deleteSoftLineBackward` | same |
| Home/End | input: value ends; textarea: line ends | line ends (`<br>` and blocks) |
| ArrowUp/Down | input: value ends; textarea: line, same column | line, same column |
| Shift+Enter | textarea: linefeed | `insertLineBreak` (`<br>`, or a linefeed where white space preserves them) |

Playwright on macOS sends `Input.dispatchKeyEvent.commands` (e.g.
`deleteWordBackward`, `moveToLeftEndOfLine`, `selectAll`, `copy`, `undo`).
Handled commands run in order in place of the key's default action; when none
is handled (`scrollPageDown`, `cancelOperation`, ...) the key's default action
runs.

Words are runs of letters, digits and `_` (with `'` and `.` inside a word);
symbols and spaces before a word are skipped, as Chromium does on macOS and
Linux. Lines are logical lines: block boundaries, `<br>` and preserved
linefeeds. Soft wrapping is not modelled.

`Selection.modify(alter, direction, granularity)` supports `move`/`extend`,
`forward`/`backward`/`left`/`right` (left-to-right text) and every
granularity (`character`, `word`, `sentence`, `line`, `paragraph`,
`lineboundary`, `sentenceboundary`, `paragraphboundary`,
`documentboundary`). It moves inside the editing host of the focus, else in
the body. Other arguments do nothing.

## Mouse

A press with `clickCount` 2 selects the word (or run of spaces or symbols)
under the pointer; 3 selects the block's contents (document text,
contenteditable) or, in a text control, the whole input value or the
textarea's line. Glyph positions inside text controls are not laid out, so
there the word is the one at the control's current caret.

## Clipboard

Copy, cut and paste fire a trusted `ClipboardEvent` (bubbles, cancelable,
composed) at the focused text control, else the element where the selection
starts, else the focused element or the body. `clipboardData` is read/write
for copy and cut and read-only (filled from the clipboard) for paste.

- copy: canceled, the handler's `setData` entries are written; else the
  selection's text (`text/plain`; block boundaries as linefeeds) and markup
  (`text/html`). A password's value is never copied.
- cut: as copy, then the selection is deleted (`deleteByCut`) in a text
  control or editable content. Outside editable content nothing is written.
- paste: unless canceled, text controls get `text/plain` (`insertFromPaste`,
  `data` set, no `dataTransfer`); editable content gets `text/html` when the
  clipboard has it (scripts, styles and metadata dropped), else the text with
  its linefeeds as line breaks (`insertFromPaste`, `data` null,
  `dataTransfer` read-only).

The clipboard is the browser context's in-memory store that
`navigator.clipboard` uses. Shortcuts and Playwright's commands are the
user's and need no permission; `navigator.clipboard.readText()` still
needs `clipboard-read`. `execCommand('copy'/'cut')` run as the shortcut
does (Chromium also requires transient user activation, which is not
tracked); `execCommand('paste')` returns false and
`queryCommandSupported('paste')` is false, as for web pages in Chromium.

`ClipboardEvent` and `CaretPosition` are made in JS on a realm's first use
(the global name, or the first clipboard event the realm sees).

## execCommand

Text controls: `insertText`, `delete`, `forwardDelete`, `insertLineBreak`
and `insertParagraph` (textarea), `selectAll`, `copy`, `cut`, `undo`, `redo`.

Editing hosts (the focused element's, else the selection's): those plus
`insertHTML`, `bold`, `italic`, `underline`, `strikethrough`, `superscript`,
`subscript` (wrapped in `b`, `i`, `u`, `strike`, `sup`, `sub`; run again on
fully formatted text, removed), `createLink`/`unlink`, `insertOrderedList`/
`insertUnorderedList` (again inside such a list: its items become `div`
lines), `formatBlock` (`<h1>`..`<h6>`, `p`, `div`, `pre`, `blockquote`, ...)
and `removeFormat`. Each fires `beforeinput`/`input` with its inputType.
Inline formatting of a collapsed selection (Chromium's typing style) is not
modelled: it returns false. `defaultParagraphSeparator` and `styleWithCSS`
are not supported.

`queryCommandState` reads computed style (bold: `font-weight` >= 600; italic;
underline and strikethrough from any ancestor's `text-decoration-line`) and
list ancestors; `queryCommandIndeterm` is true when only part of the
selection has the style; `queryCommandValue` gives `formatBlock`'s tag,
`fontName`/`foreColor`/`backColor` computed values, `"true"`/`"false"` for
states and `defaultParagraphSeparator` "div"; `queryCommandEnabled` needs an
editable target (copy: a selection; undo/redo: a step).

## Undo

One history per document. A step is what its target looked like before an
edit: a text control's value and selection, an editing host's markup and
caret. Consecutive typing at the caret the last typing left is one step;
any other edit or caret move starts another. Undo and redo fire cancelable
`beforeinput` `historyUndo`/`historyRedo`, restore, then fire `input`. At
most 100 steps are kept. Script changes to the content are not undo steps.

## Caret from point

`document.caretRangeFromPoint(x, y)` returns a collapsed Range and
`document.caretPositionFromPoint(x, y, {shadowRoots})` a `CaretPosition`
(`offsetNode`, `offset`, `getClientRect()`) at the character boundary
nearest to the point in the hit element's laid-out text; beside a replaced
element or control, the position before or after it; in an element without
text, its start; null outside the viewport. Positions in shadow trees not
listed in `shadowRoots` move out to their hosts (caretRangeFromPoint lists
none).
