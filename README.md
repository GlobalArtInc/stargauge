# stargauge

Track the stars of any public GitHub repository: statistics, terminal charts and an offline HTML report.
For repositories you administer or collaborate on it also shows who starred and unstarred.
Runs on Linux, macOS and Windows.

stargauge is a Rust rewrite of [Shotstars](https://github.com/snooppr/shotstars) by Snooppr.
Licensed under GPL-3.0-or-later, the same licence as the original. See [LICENSE](LICENSE) and [NOTICE](NOTICE).

## Two modes

Since July 2026 GitHub opens the list of stargazers only to the admins and collaborators of a repository.
stargauge picks the source for every scan on its own:

| | Stargazer list | Public star history |
|---|---|---|
| When | A token gives access to the list (your repositories) | Everything else, including no token at all |
| New and gone stars | Names, with the full history of both and users who keep starring and unstarring | Numbers only; the number of gone stars is a lower estimate |
| Cross-users, user JSON | Yes | No |
| Statistics, trend, fake-star indicators | Yes | Yes |
| Charts | Six, including stars by hour | Five, no stars by hour |
| Requests per scan | 1 per 100 stars | 1 per 30 weeks of repository age (10 years: 18) |

## What it does

- Statistics: peak day, trend, best and worst weekday and month, quartiles by month and year, the longest stretch
  without new stars, the change over the last 30 days, and indicators of aggressive marketing and fake stars.
- Charts in the terminal and interactive charts in an offline HTML page (zoom, tooltips, save as PNG,
  night theme from 8 pm to 3 am).
- New and gone stars between scans (names in list mode, numbers in public mode).
- Cross-users (list mode): accounts that starred several of the repositories you track.
- An HTML report that opens in your browser after every rescan.

## Install

Prebuilt binaries for Linux (x86_64 and aarch64, static musl), macOS (Intel and Apple silicon) and Windows 10+ (x64)
are attached to every [release](https://github.com/GlobalArtInc/stargauge/releases), together with `SHA256SUMS`.

The binaries are not code-signed. On macOS remove the quarantine flag after download:

```
xattr -d com.apple.quarantine ./stargauge
```

On Windows, SmartScreen may ask for confirmation on first run.

From source (Rust 1.88 or newer):

```
cargo install --locked --git https://github.com/GlobalArtInc/stargauge
```

Android/Termux: `pkg install rust`, then the same command.

## Usage

```
stargauge                              ask for a repository
stargauge owner/repo                   scan a repository
stargauge https://github.com/owner/repo
stargauge history                      pick a previously scanned repository by number
stargauge clear [--yes]                delete scan data (the config file is kept)
stargauge paths                        print the data directory and the config file
stargauge --no-open owner/repo         do not open the report in the browser
```

The first scan of a repository stores a baseline. From the second scan on, stargauge reports what changed since
the previous scan and writes the HTML report.

## GitHub token and limits

Without a token GitHub allows 60 API requests per hour per IP address. The public star history costs one request
per 30 weeks of repository age, so a repository of 10 years takes 18 requests and there is no limit on the number
of stars.

A free personal access token raises the limit to 5,000 requests per hour. If the account behind the token is an
admin or a collaborator of the repository, stargauge reads the stargazer list (up to 40,000 stars, GitHub does not
page further). For every other repository, and for repositories above 40,000 stars, it falls back to the public
history and says so.

Put the token in the `STARGAUGE_TOKEN` environment variable or in `config.toml`:

```
[github]
token = "ghp_..."
```

The token is sent only to `api.github.com`, never printed, and on Unix the config file is kept at mode 0600.

## Where the data lives

| OS | Data | Config |
|---|---|---|
| Linux | `~/.local/share/stargauge` | `~/.config/stargauge/config.toml` |
| macOS | `~/Library/Application Support/stargauge` | same directory |
| Windows | `%LOCALAPPDATA%\stargauge` | same directory |

`STARGAUGE_HOME` overrides both. Each repository gets `repos/{owner}/{repo}/state.json` plus the generated pages.
In list mode `state.json` holds the last snapshot of logins and the log of added and removed stars; in public mode
it holds the daily star counts and the log of changes as numbers. Everything except `state.json` can be deleted and
is regenerated on the next scan. `stargauge clear` removes only what stargauge created.

To check the report on a quiet repository in list mode, remove a few lines from `logins` in `state.json` and scan
again; in public mode lower a number in `star_days`.

## Coming from Shotstars

stargauge starts with an empty data directory and does not read `~/.ShotStars` or `%LOCALAPPDATA%\ShotStars`.
Shotstars keeps data per repository name only, so `alice/tool` and `bob/tool` overwrite each other there.
The gone-stars history exists in Shotstars only as generated HTML. stargauge keeps it as structured data.

Shotstars needs the stargazer list for every scan, so it no longer works on repositories you do not administer.
stargauge falls back to the public history there.

Other differences: Windows 10 or newer is required, terminal charts work on Windows, the report uses relative links
instead of `file://`, a token is also accepted from the environment, and the donation splash is gone.

## Licence

GPL-3.0-or-later. Copyright (C) 2024 Snooppr, (C) 2026 GlobalArt. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
