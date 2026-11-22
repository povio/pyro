# Applying bold to bare text next to B and STRONG

Tested source `36b22e81f709af396814ca4d506895d9982ebc93`, tree `a49a628e1b11271ac9cdb499ea302a2a4549ce20`, executable SHA-256 `e295741785be49d37d490f52a87a199dbe26aa2dc349ea8b5af5bd6561fa30fe`. Exact parent `59c4ba28fbe12925598b0949f120d9ecb303148b` contains integrated loader/parser changes. Its read-only executable SHA-256 `249180360d7d21c0a5f1c92c0e9512c9705bbb052ffb79a9a87facbd1ddc8ad1` was shared with the ordinary lane. The signed WPT arm retains its original Chrome 158.0.8127.37 provenance; the direct controls measured signed Chrome and this exact parent.

Applying bold to a single selected text run in legacy mode now joins its newly created B wrapper with immediately adjacent unadorned B siblings. The right-hand wrapper survives each join, with the children moved in document order. Text endpoints remain on the original text nodes. STRONG, attributed B, and CSS-mode formatting retain their distinct elements.

Independent QA compared 44 WPT pages and 33,326 matched assertions. All five frozen signed targets recovered, with zero losses and unchanged test membership. [Changed assertions](changed-names.json)

All 192 main and 128 neighboring cases matched signed markup, computed font weight/color/style, endpoints and tracked connectivity after the first toggle. On repeat, all 192 main cases matched markup, glyphs and endpoints. In the neighboring suite repeat markup matched in 108/128, glyphs in 124/128 and endpoints in 118/128. No repeat field lost a prior signed match.

Final undo matched live markup, glyphs and endpoints in all 368 observations, including 48 fresh history probes. Original connectivity vectors remained different in all 368, as on the parent. The final textContent vector of the tracked original nodes newly differs in ten main and ten neighboring fixtures. Six observations in the history suite repeat main fixture IDs. The parent retained the old wrapper contents on detached nodes because it had not merged the wrappers. The candidate moves those children as Chrome does on the first toggle; host reconstruction on undo leaves the original objects disconnected, while Chrome restores their contents and reconnects them. The differences are counted separately from live restored host markup. [Original wrapper observations by fixture and stage](original-wrapper-text.json.gz)

The native numeric-bold removal test passed. All 44 numeric-bold and 80 createLink control records matched the exact parent. All 10,092 command multitest and 13,060 static neighboring assertion statuses were unchanged.
