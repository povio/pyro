# FormData

`FormData` holds an ordered entry list of `(name, string | File)`.

- `new FormData()`, `new FormData(form)` and `new FormData(form, submitter)`
  use the HTML form data set algorithm (successful controls, checked
  checkboxes/radios, selected options, CRLF-normalized textarea values, the
  submitter's name/value, file inputs). A non-form argument throws TypeError;
  a submitter that is not owned by the form throws NotFoundError.
- `append/set/delete/get/getAll/has`; names and string values use USVString
  conversion. Blob values become `File`s named `"blob"` (or the given
  filename); `File` values keep their identity, so `get()` returns the same
  object.
- `entries/keys/values/@@iterator/forEach`. Iterators snapshot the entry list
  when created; later mutations are not reflected in an existing iterator
  (documented deviation from the live iteration in the spec).
- As a `fetch`/`Response`/`XMLHttpRequest.send` body it is serialized as
  `multipart/form-data` with a generated boundary in the Content-Type.

Form submission encodes in the form's encoding: the first usable
`accept-charset` label, else the document's encoding, with UTF-16 and
`replacement` mapped to UTF-8. Characters the encoding cannot represent are sent
as `&#NNN;` and a hidden `_charset_` control reports the encoding's name.
`FormData` bodies passed to `fetch`/XHR stay UTF-8.

Constructing a form's entry list (`new FormData(form)` and every submission
but `method=dialog`) fires a trusted `formdata` event (`FormDataEvent`,
bubbles, not cancelable) at the form whose `formData` holds the controls'
entries; what listeners append or delete is in the resulting FormData and in
the submission (whose names and text values get CRLF line breaks). While the
event is dispatched the form is constructing its entry list: a nested
`new FormData(form)` throws InvalidStateError and a nested submission does
nothing. The event's FormData is a separate object (as in Chrome); for a
submission it is made in the main world. The event is only worked out once a
`formdata` listener was ever added in the document's tree (a flag set by
`addEventListener`/`onformdata`), so forms on other pages pay nothing.

Not implemented: `File`-name/Content-Type escaping beyond quoting `"`, CR
and LF.
