# Changelog

## 0.1.0

Rust rewrite of Shotstars v4.13 by Snooppr, renamed to stargauge.

### Added

- Public star history mode. GitHub opened the stargazer list only to admins and collaborators in July 2026, so for
  every other repository (and without a token) stargauge reads the public star history instead: statistics, trend,
  fake-star indicators, five charts and the numbers of new and gone stars (gone stars are a lower estimate).
  The source is chosen automatically for every scan; with access to the list the full behaviour stays.

### Changed

- Single static binary for Linux, macOS and Windows; no Python or runtime needed.
- New command line: `stargauge [REPO]`, `history`, `clear`, `paths`, `--no-open`.
- Data is stored per `owner/repo` in one atomically written `state.json` (snapshot and event log) instead of
  `new.txt`, `old.txt` and `history.json`. The history of gone stars no longer depends on parsing generated HTML.
- Configuration moved to `config.toml`; the token can also come from `STARGAUGE_TOKEN`.
- HTML charts use Apache ECharts instead of Plotly; the theme is chosen when the page is opened.
- Generated pages link to each other with relative paths instead of `file://`.
- Windows 10 or newer is required.

### Fixed

- Crash when the star-count deviation is exactly 4, 6, 11, 17 or 35.
- Repositories with equal names under different owners shared one directory and were compared with each other.
- A repository URL ending with `/` could delete all scan data and the config file on a 404.
- Repositories whose name contains "shotstars" could not be scanned.
- Repositories with zero stars crashed the scan.
- Any network or server error was reported as a rate limit; a bad token was reported as a missing repository.
- Stargazers added during a scan were missed or duplicated.
- Ctrl-C killed the process with SIGKILL and could leave half-written files.
- Reports with a path containing spaces or quotes did not open correctly on Windows.
- Terminal width was read at start-up and crashed error messages when output was piped.
- Peak day, best and worst month and quartile order depended on request completion order.
- Windows always waited for ENTER, and terminal charts were disabled there.
- Report HTML had two `<body>` elements.

### Removed

- The donation splash screen and the donation link to the upstream author. Credit to Snooppr stays in the banner,
  the report footer, `--version` and `NOTICE`.
- The `pepy.tech` easter egg.
