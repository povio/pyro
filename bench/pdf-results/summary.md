# Per case (final run, medians of 3)

Chrome = chrome-fixed (page box shadow removed). LP flow = lp-dom geometry; LP Blitz = LIGHTNINGPANDA_GEOMETRY=blitz.

| case | pages Chrome / LP flow / LP Blitz | paginate ms Chrome / flow / Blitz | Blitz layouts | printToPDF ms Chrome / LP | pixel diff % flow / Blitz | text overlap flow / Blitz | LP vertical overflow px flow / Blitz |
|---|---|---|---|---|---|---|---|
| invoice-preview | 1 / 1 / 1 | 46 / 67 / 86 | 3 | 97 / 28 | 4.9 / 4.9 | 1.000 / 1.000 | 0 / 0 |
| invoice-final | 1 / 1 / 1 | 43 / 65 / 85 | 3 | 99 / 28 | 4.9 / 4.9 | 1.000 / 1.000 | 0 / 0 |
| invoice-final-copy | 1 / 1 / 1 | 46 / 66 / 86 | 3 | 98 / 28 | 4.9 / 4.9 | 1.000 / 1.000 | 0 / 0 |
| invoice-office-kombi-kargo-english | 1 / 1 / 1 | 40 / 56 / 76 | 3 | 75 / 16 | 4.4 / 4.4 | 0.981 / 0.981 | 0 / 0 |
| invoice-office-kombi-kargo-german | 1 / 1 / 1 | 46 / 54 / 73 | 3 | 79 / 16 | 4.5 / 4.5 | 0.979 / 0.979 | 0 / 0 |
| invoice-office-ole-hamburg-english | 1 / 1 / 1 | 41 / 56 / 74 | 3 | 74 / 17 | 4.8 / 4.8 | 0.991 / 0.991 | 0 / 0 |
| invoice-office-ole-hamburg-german | 1 / 1 / 1 | 42 / 57 / 72 | 3 | 77 / 17 | 4.9 / 4.9 | 0.989 / 0.989 | 0 / 0 |
| invoice-office-tos-budapest-english | 1 / 1 / 1 | 48 / 62 / 77 | 3 | 77 / 17 | 4.9 / 4.9 | 0.971 / 0.971 | 0 / 0 |
| invoice-office-tos-budapest-hungarian | 1 / 1 / 1 | 48 / 62 / 107 | 3 | 78 / 17 | 5.1 / 5.1 | 0.962 / 0.962 | 0 / 0 |
| invoice-office-tos-houston-english | 1 / 1 / 1 | 44 / 60 / 75 | 3 | 76 / 17 | 4.0 / 4.0 | 0.984 / 0.984 | 0 / 0 |
| invoice-office-tos-koper-english | 1 / 1 / 1 | 43 / 59 / 77 | 3 | 74 / 17 | 4.1 / 4.1 | 0.985 / 0.985 | 0 / 0 |
| invoice-office-tos-prague-czech | 1 / 1 / 1 | 47 / 41 / 109 | 3 | 83 / 18 | 4.7 / 4.7 | 0.988 / 0.988 | 0 / 0 |
| invoice-office-tos-prague-english | 1 / 1 / 1 | 48 / 58 / 78 | 3 | 76 / 18 | 4.7 / 4.7 | 0.983 / 0.983 | 0 / 0 |
| invoice-office-tos-vienna-english | 1 / 1 / 1 | 46 / 60 / 79 | 3 | 78 / 17 | 4.7 / 4.7 | 0.988 / 0.988 | 0 / 0 |
| invoice-office-tos-vienna-german | 1 / 1 / 1 | 48 / 42 / 77 | 3 | 78 / 17 | 4.8 / 4.8 | 0.987 / 0.987 | 0 / 0 |
| invoice-bank-footer-tos-budapest-hungarian-bank-huf | 1 / 1 / 1 | 45 / 61 / 95 | 3 | 78 / 20 | 4.8 / 4.8 | 0.979 / 0.979 | 0 / 0 |
| invoice-bank-footer-tos-budapest-english-bank-main | 1 / 1 / 1 | 47 / 93 / 80 | 3 | 80 / 18 | 4.6 / 4.6 | 0.986 / 0.986 | 0 / 0 |
| invoice-bank-footer-tos-vienna-english-bank-factoring | 1 / 1 / 1 | 44 / 62 / 77 | 3 | 77 / 17 | 4.9 / 4.9 | 0.989 / 0.989 | 0 / 0 |
| invoice-bank-footer-tos-vienna-english-bank-regular | 1 / 1 / 1 | 45 / 62 / 77 | 3 | 77 / 17 | 4.6 / 4.6 | 0.988 / 0.988 | 0 / 0 |
| bmd-report | 1 / 1 / 1 | 35 / 44 / 51 | 3 | 65 / 12 | 0.8 / 0.8 | 0.947 / 0.947 | 0 / 0 |
| bl-instructions-cargo-alignment-preview | 1 / 1 / 1 | 40 / 52 / 69 | 3 | 69 / 16 | 4.6 / 4.6 | 1.000 / 1.000 | 0 / 0 |
| bl-instructions-cargo-alignment-final | 1 / 1 / 1 | 40 / 53 / 71 | 3 | 65 / 16 | 4.6 / 4.6 | 1.000 / 1.000 | 0 / 0 |
| house-bl-final-historical-b | 2 / 2 / 2 | 44 / 100 / 110 | 6 | 78 / 20 | 3.9 / 3.9 | 0.998 / 0.998 | 0 / 0 |
| house-bl-final-copy-historical-b | 2 / 2 / 2 | 43 / 56 / 105 | 6 | 77 / 20 | 3.9 / 3.9 | 0.998 / 0.998 | 0 / 0 |
| house-bl-final-overflowing-table | 4 / 3 / 4 | 75 / 190 / 581 | 147 | 92 / 32 | 29.7 / 5.5 | 0.922 / 1.000 | 0 / 0 |
| quote-document | 2 / 2 / 2 | 60 / 118 / 157 | 20 | 109 / 33 | 4.7 / 4.7 | 0.981 / 0.981 | 0 / 0 |
| templated-document-cargo-alignment-preview | 1 / 1 / 1 | 38 / 35 / 59 | 3 | 65 / 14 | 2.9 / 2.9 | 1.000 / 1.000 | 0 / 0 |
| templated-document-cargo-alignment-final | 1 / 1 / 1 | 37 / 44 / 58 | 3 | 64 / 13 | 2.9 / 2.9 | 0.987 / 0.987 | 0 / 0 |
| booking-request-preview | 1 / 1 / 1 | 40 / 51 / 72 | 3 | 72 / 19 | 5.6 / 5.6 | 0.991 / 0.991 | 0 / 0 |
| booking-request-final | 1 / 1 / 1 | 42 / 54 / 70 | 3 | 71 / 19 | 5.6 / 5.6 | 0.991 / 0.991 | 0 / 0 |
| booking-request-sparse | 1 / 1 / 1 | 31 / 41 / 54 | 3 | 62 / 12 | 2.2 / 2.2 | 1.000 / 1.000 | 0 / 0 |
| booking-request-overflow | 10 / 10 / 9 | 116 / 259 / 1051 | 164 | 122 / 69 | 6.4 / 16.1 | 0.994 / 0.985 | 0 / 0 |
| booking-confirmation-preview | 1 / 1 / 1 | 43 / 52 / 72 | 3 | 70 / 20 | 5.6 / 5.6 | 0.991 / 0.991 | 0 / 0 |
| booking-confirmation-final | 1 / 1 / 1 | 42 / 52 / 70 | 3 | 71 / 19 | 5.6 / 5.6 | 0.991 / 0.991 | 0 / 0 |
| booking-confirmation-sparse | 1 / 1 / 1 | 33 / 40 / 53 | 3 | 60 / 12 | 2.2 / 2.2 | 1.000 / 1.000 | 0 / 0 |
| booking-confirmation-overflow | 10 / 10 / 9 | 94 / 295 / 1074 | 164 | 118 / 69 | 6.5 / 16.2 | 0.994 / 0.985 | 0 / 0 |
| vermas-preview | 1 / 1 / 1 | 37 / 43 / 59 | 3 | 68 / 18 | 2.7 / 2.7 | 0.980 / 0.980 | 0 / 0 |
| vermas-final | 1 / 1 / 1 | 41 / 45 / 61 | 3 | 68 / 17 | 2.7 / 2.7 | 0.980 / 0.980 | 0 / 0 |
| vermas-sparse | 1 / 1 / 1 | 32 / 38 / 49 | 3 | 59 / 11 | 1.2 / 1.2 | 1.000 / 1.000 | 0 / 0 |
| vermas-overflow | 2 / 2 / 2 | 50 / 78 / 148 | 46 | 74 / 24 | 4.2 / 4.7 | 0.833 / 0.991 | 324 / 0 |
| cmr-reference | 1 / 1 / 1 | 42 / 55 / 75 | 3 | 112 / 18 | 5.6 / 5.6 | 0.902 / 0.902 | 0 / 0 |
| cmr-populated | 1 / 1 / 1 | 42 / 56 / 77 | 3 | 110 / 18 | 4.9 / 4.9 | 0.899 / 0.899 | 0 / 0 |
| cmr-overflow | 14 / 14 / 14 | 298 / 465 / 3605 | 2544 | 637 / 39 | 4.4 / 4.4 | 0.991 / 0.991 | 0 / 0 |
| cmr-long-address | 1 / 1 / 1 | 43 / 55 / 76 | 3 | 111 / 18 | 5.0 / 5.0 | 0.903 / 0.903 | 0 / 0 |
