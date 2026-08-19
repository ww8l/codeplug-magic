# sample-data

Fixtures for the importer tests. **Everything here is hand-authored.** No file
in this directory is a capture, a dump, or an export from a live service, and
none of it should ever be replaced with one — publishing this repo publishes
whatever sits in here.

| File | Feeds | Notes |
| --- | --- | --- |
| `repeaterbook-full-sample.json` | `parses_sample_repeaterbook_json` | The RepeaterBook "Full Data" JSON export *shape*, with invented records. |
| `repeaterbook-sample.csv` | `parses_sample_repeaterbook_csv` | The RepeaterBook CSV export shape, with invented records. |
| `talkgroups-sample.csv` | `parse_talkgroup_csv` test | Invented talkgroups, including two malformed rows the parser must skip. |

## Rules for anything added here

- **Call signs come from the Q prefix block** (`QQ0AAA`, `QQ0BBB`, …). ITU
  reserves prefixes beginning with Q for Q-codes, so they are never issued to a
  station and can never collide with a real licensee. A call sign resolves
  through the national licence database to a mailing address, which is why a
  plausible-looking one is not good enough.
- **Places and coordinates are invented** — Anytown, Testville, Sampleton,
  Nowhere, on round lat/lon. Real 8-decimal site coordinates identify a real
  repeater site.
- **No export envelope from a real service.** A `generated_at` stamp or an
  account-scoped template id is provenance evidence that this came out of
  someone's account. Keep the field *names* the importer parses; drop the rest.
- **DMR IDs sit above the assigned user range** (`9999xxx`), so they cannot
  match an issued ID. Same reasoning as the `dmr_users.rs` fixtures.

`repeaterbook-full-sample.json` replaced a verbatim RepeaterBook export that had
been checked in since early development (#56). It carried RepeaterBook's own
export envelope plus 91 real records with call signs and 8-decimal site
coordinates. RepeaterBook is the app's primary import source; republishing its
export is not worth the import privilege the app depends on.
