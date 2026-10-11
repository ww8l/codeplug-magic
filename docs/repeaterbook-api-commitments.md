# RepeaterBook API: what we told RepeaterBook

This file holds the answers submitted to RepeaterBook's API app application
(distributed-app lane) on 2026-09-18. It is a resubmission; the first
application was sent back asking for more detail.

**Every answer below is a promise about how CodePlug Magic behaves.** None of
the integration was built when they were written. If RepeaterBook approves the
app, the import feature has to make every one of them true. The checklist
comes first; the submitted text, verbatim, follows it. If the code and this
file ever disagree, either change the code or tell RepeaterBook before
shipping. Don't quietly edit this file to match the code.

⚠ This is the text as it was filled into the form. Tim reviewed it before
submitting; if he changed anything there, update this file to match what was
actually sent.

## Checklist for the implementation

Identity and token
- [ ] Every request sends exactly `CodePlugMagic (+https://github.com/ww8l/codeplug-magic; WW8L; tim@ww8l.com)`. No version number.
- [ ] Per-user `rbuapp_` token, pasted once, stored in the OS credential store
      (Keychain / Credential Manager / Secret Service), e.g. the `keyring` crate.
- [ ] Token never in SQLite, settings, logs or backup files.
- [ ] Token sent only over HTTPS, only in the `X-RB-App-Token` header, never in a URL.
- [ ] Logs record only request type, search area, HTTP status and timestamp.
- [ ] The user can remove the token in the app.
- [ ] Token setup screen links to RepeaterBook's API Apps & Tokens page.

Requests and limits
- [ ] One search = one `api.export` request for one area (one US state or
      Canadian province, or one country outside North America), with optional
      band and mode filters. No automatic paging.
- [ ] More than 1,000 results → import nothing, ask the user to narrow filters.
- [ ] Nothing saved until the user selects rows; at most 500 per import.
- [ ] `api.export` (export.php) for US states and Canadian provinces; `api.export_row`
      (exportROW.php) for a country outside North America. A single-record refresh is
      one request, narrowed by the record's area and call sign, made only when the user
      asks to refresh that record.
- [ ] One request in flight; at least 2 s apart; at most 20 per hour, enforced.
- [ ] Import button disabled while a request runs.
- [ ] An identical search within 10 minutes is served from memory (never disk).
- [ ] 429/5xx: honor Retry-After, else wait 30 s / 60 s / 120 s, stop after 3
      retries; the user cannot bypass the wait.
- [ ] 401/403: stop at once, no retry, ask the user to check the token.
- [ ] No background polling, scheduled sync, crawling, or looping over areas.

Storage, retention and deletion
- [ ] API records carry a source distinct from file-imported RepeaterBook rows
      (today those are `source = 'repeaterbook'` with a synthetic
      CALLSIGN|FREQ|STATE|CITY `repeaterbook_id`). Without this, none of the
      limits below can tell them apart.
- [ ] Each record stores its import date; re-import updates by RepeaterBook's
      own identifier instead of duplicating.
- [ ] Records not refreshed within 12 months are flagged, and the user is
      prompted to refresh or delete them.
- [ ] Single-record delete, and a "Remove RepeaterBook data" action that deletes
      every API record AND the stored token in one step.

Exports and backups
- [ ] API records are excluded from the channel export file
      (`export_channels`, `src-tauri/src/commands/channel_io.rs`), and the
      user is told how many were left out.
- [ ] Radio programming output carries only the channels in a codeplug for that
      radio, within its channel capacity.
- [ ] "RepeaterBook.com" in the Comment field of RepeaterBook records in export
      formats that have one.
- [ ] The whole-database backup (`export_database`, `src-tauri/src/commands/backup.rs`)
      never contains the token.
- [ ] No feature exports RepeaterBook records as a list or dataset.

Attribution ("Data courtesy of RepeaterBook.com", linked to https://www.repeaterbook.com)
- [ ] The search/import dialog, visible the whole time.
- [ ] The summary after every import.
- [ ] Channel library: Source column reads "RepeaterBook"; the detail view shows
      the attribution line, the link and the import date.
- [ ] The About screen, the README, and the import feature's docs.

Also attested on the form (checkboxes): display the attribution and link back
where practical; do not mirror, redistribute, bulk-export, re-serve, or build
another directory, dataset, service or API from the data without written
permission.

## Form values as submitted

| Field | Value |
|---|---|
| Contact Name / Call Sign | Tim Annable, WW8L |
| Contact Email | tim@ww8l.com |
| Project / Application Name | CodePlug Magic |
| Project website | https://github.com/ww8l/codeplug-magic |
| Application User-Agent | `CodePlugMagic (+https://github.com/ww8l/codeplug-magic; WW8L; tim@ww8l.com)` |
| Primary RepeaterBook use | Personal radio programming |
| Who can use it? | Public users |
| Estimated users | 25 |
| Commercial status | Non-commercial |
| Implementation status | Planned |
| Source availability | Open source |
| Project Categories | Open-Source, Not for Profit, Hobby/Personal |

## Application review details

CodePlug Magic is a free, open-source (GPL-3.0) desktop application for amateur radio operators: https://github.com/ww8l/codeplug-magic. It runs on the user's own computer (Windows, macOS, Linux). It keeps a local library of channels, builds codeplugs from it, and programs the user's own radios over USB, serial cable, or the radio's microSD card. Supported radios include models from AnyTone, Kenwood, Icom, Yaesu, TIDRADIO, Baofeng and Binteradio.

Users are individual hams programming their own radios. Today they export a CSV or JSON file from the RepeaterBook website and import it into the app. The API would replace that manual export and import step. Each user creates their own free RepeaterBook account and generates their own app-bound rbuapp_ token. Requests go directly from the user's computer to RepeaterBook. There is no server of ours in between, and no data is shared between users.

This is a resubmission. The previous review asked for more detail on attribution and link-back, maximum returned and selected records, request spacing and 429 backoff, cache/retention/deletion, how records in the local library, exports and backups stay limited to radio programming, and the final User-Agent. Each is answered in its own section below. In summary:
- Final User-Agent, sent exactly and unchanged between releases: CodePlugMagic (+https://github.com/ww8l/codeplug-magic; WW8L; tim@ww8l.com)
- At most 1,000 records displayed per search, and at most 500 selected per import.
- At least 2 seconds between requests, no more than 20 requests per hour per user, one request in flight at a time, and 429/5xx backoff honoring Retry-After.
- RepeaterBook records leave the app only as radio programming output. They are excluded from the app's general channel-export file, and there is no feature for exporting them as a list or dataset.

The API integration is not built yet (Implementation status: Planned). The answers here are the design it will be built to.

Developer: Tim Annable, WW8L (tim@ww8l.com).

## API workflow and data fields

Workflow:
1. The user opens "Import from RepeaterBook" and picks one area: one US state or Canadian province, or one country outside North America. Before sending, they can narrow by band and by mode (FM, DMR, D-STAR, System Fusion, NXDN, P25).
2. The app sends one api.export request for that area and those filters. It does not automatically page through further results. If the search matches more than 1,000 records, none are imported and the user is asked to narrow the filters.
3. The results appear in a preview table. Nothing is saved until the user selects records, and at most 500 can be selected in one import.
4. Selected records are saved to the user's local channel library, where the user assigns them to codeplugs for specific radios and programs those radios.
5. api.export serves searches in a US state or Canadian province, and api.export_row serves searches in a country outside North America. To refresh one record the user already has, and only when the user asks, the app sends one request narrowed to that record's area and call sign.

Data fields used, and only for building a channel:
- Output frequency, input frequency / offset
- Callsign
- Uplink and downlink tone (CTCSS / DCS)
- Modes (FM analog, DMR with color code, D-STAR, System Fusion, NXDN, P25) and digital access notes
- Location (city / landmark), county, state or province, country
- Latitude and longitude, used to sort channels by distance from the user
- Use (open / closed) and operational status, so off-air or closed repeaters can be skipped
- RepeaterBook's own identifiers for the record, used only to update a record on re-import instead of duplicating it
- Notes, shown to the user in the channel's detail view

The app does not ask for or store personal data about repeater owners or trustees beyond the repeater callsign.

This is not a proxy, mirror, directory, map, bulk download or secondary API. The app has no server component. It makes requests only for the user who is sitting at it, only for their own radios, and never serves results to anyone else.

## Relationship to RepeaterBook

CodePlug Magic is a radio programming tool, not a repeater directory. It does not compete with RepeaterBook's website, mobile apps or directory. It has no browsing, mapping, repeater search across regions, trip planning or repeater detail pages. RepeaterBook data enters the app only as channels for a user to program into their own radio.

Users still rely on RepeaterBook for discovery. The in-app dialog says "Data courtesy of RepeaterBook.com" and links to RepeaterBook, and each imported record keeps RepeaterBook as its source. Corrections are sent to RepeaterBook, not handled in the app. The app does not accept or relay repeater edits.

The API replaces a step users already do by hand: export a CSV or JSON file from repeaterbook.com and import it. Moving that step to the API puts every request under the user's own app-bound token, the published rate limits and an identifiable User-Agent, instead of an untracked file download. The app already reads RepeaterBook's export format today.

## Credential handling and abuse prevention

Each user creates their own free RepeaterBook account and generates their own app-bound rbuapp_ token from the RepeaterBook dashboard. They paste it into CodePlug Magic once. No shared or app_ token exists. Nothing is embedded in the app, the installer, or the public source code.

Storage: the token is stored in the operating system's credential store (macOS Keychain, Windows Credential Manager, or Linux Secret Service). It is never written to the app's SQLite database, its settings, its logs, or its backup files, so a database backup the user makes never contains it. Only that user's OS account can read it.

Transmission: the token is sent only to RepeaterBook, only over HTTPS, and only in the X-RB-App-Token request header. It is never put in a URL or query string.

Logging: the app never logs the token. Its logs record only the request type, the search area, the HTTP status and a timestamp.

Revocation: the user can remove the token from the app at any time. "Remove RepeaterBook data" deletes the stored token together with every record that came from the API. The user can also rotate or revoke the token on RepeaterBook. A 401 or 403 response stops all requests immediately, with no retry, and asks the user to check or replace their token.

Abuse prevention: requests are made only when the user starts a search. There is no background polling, scheduled sync, crawling or bulk download, and no feature for looping over many areas. The search and selection limits and the rate controls described in the other sections apply to every request.

## Rate and abuse controls

- One request in flight at a time per user, never concurrent.
- At least 2 seconds between requests.
- No more than 20 requests per hour per user, enforced by the app.
- Each search is one request for one user-chosen area. The app does not automatically page through further results.
- At most 1,000 records displayed per search. If a search matches more, nothing is imported and the user is asked to narrow the band or mode filters.
- At most 500 records selected per import.
- The Import button is disabled while a request is running.
- An identical search repeated within 10 minutes reuses the result already in memory instead of calling the API again.
- Requests happen only when the user starts a search. There is no background polling, scheduled sync or crawling.

HTTP 429 and 5xx: the app honors Retry-After when it is present. Otherwise it waits 30 seconds, then 60, then 120. After 3 failed retries it stops and tells the user to try again later. The user cannot bypass the wait.

HTTP 401 and 403: the app stops immediately without retrying and asks the user to check their token.

Every request sends exactly this User-Agent: CodePlugMagic (+https://github.com/ww8l/codeplug-magic; WW8L; tim@ww8l.com)

## Cache and retention policy

Response cache: the only cache is the 10-minute, in-memory search result described under rate controls. It is never written to disk and is lost when the app closes.

Retention: records the user selects are stored in their local channel library, a SQLite database on their own computer, so they can be programmed into their own radios. Nothing is uploaded, synced or shared with other users or with the developer. Each record is marked with RepeaterBook as its source and the date it was imported. Re-running an import updates existing records by their RepeaterBook identifier instead of duplicating them. Records not refreshed within 12 months are flagged, and the user is prompted to refresh or delete them.

Deletion: the user can delete any single record at any time. "Remove RepeaterBook data" deletes every record that came from the RepeaterBook API, and the stored token, in one step.

How RepeaterBook-derived records stay limited to the user's radio-programming workflow:
- Local library: the library exists to build codeplugs for the user's own radios. It is not exposed to any network service, API or other user.
- Exports: RepeaterBook records leave the app only as radio programming output, meaning a codeplug file for a specific radio model or a direct write to the user's radio. That output contains only the channels the user has placed in a codeplug for that radio, limited to that radio's channel capacity, and each record is attributed to RepeaterBook.com where the format has a comment field. The app's general "export channels" file, which moves a library between installs, leaves out records that came from the RepeaterBook API and tells the user how many were left out. There is no feature for exporting RepeaterBook records as a list or dataset.
- Backups: the app's whole-database backup exists only to restore the user's own installation after a disk failure or a move to a new computer. It is a single private file written where the user chooses. It never contains the token, is not uploaded anywhere, and is not offered as a way to share data. A restored backup brings back only the user's own records, still marked with their source and import date.

## Attribution and link-back plan

The text "Data courtesy of RepeaterBook.com", linked to https://www.repeaterbook.com, appears in these places:
1. The RepeaterBook search and import dialog, visible the whole time the user searches and previews results.
2. The summary shown after every import.
3. The channel library. Every record from RepeaterBook shows "RepeaterBook" in its Source column, and its detail view shows the attribution line, the link and the import date.
4. Radio export files: the Comment field of every RepeaterBook record reads "RepeaterBook.com" in formats that have a comment field.
5. The app's About screen, the README, and the documentation for the import feature.

The token setup screen links to RepeaterBook's API Apps & Tokens page so users create and manage their token on RepeaterBook itself.
