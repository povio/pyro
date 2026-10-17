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

Not implemented: the `formdata` event on form construction and
`File`-name/Content-Type escaping beyond quoting `"`, CR and LF.
