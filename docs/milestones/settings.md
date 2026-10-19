# Milestone: settings

Planned 2026-10-12, after the refresh panel (`refresh-panel.md`) and
before onboarding (`onboarding.md`) and EPG sources (`epg-sources.md`),
which both save through it. Tickets in `.scratch/settings/`.

## Why now

`config.toml` (SPEC 4.2) is written by hand and read once at start. A
change means editing the file and restarting, and a published app cannot
ask that of its users. The file-writing mechanism here is also what the
providers form and the EPG sources form save through.

## Requirements

- **The files stay the only source of truth.** The app edits
  `config.toml` and `credentials.toml` in place with `toml_edit`,
  keeping comments, order and layout. No settings in SQLite, no second
  file.
- **Managed files:** a file that is read-only to the user or a symlink
  into the Nix store makes its sections read-only: values show, a note
  says why (`config.toml is managed by Nix; edit it there`), nothing
  saves.
- **Live apply:** a saved value takes effect without a restart, each
  where it is used: `refresh_hours` re-arms the check, `mpv_path` and
  `extra_mpv_args` the next mpv spawn (the one playing keeps going), the
  directories, `ffmpeg_path` and padding the next recording or download.
  The app does not watch the files; it reads them at start and after its
  own saves.
- **Settings view:** `,` opens it, full-window like the account dashboard
  and the Recordings view; Esc, `,` or `q` closes. Also from the guide
  grid, and by a gear on the status bar for the mouse. Sections on the
  left: Providers, General, Player, Recording, Downloads, EPG (Providers
  moved first on 2026-10-12, after onboarding ticket 01); the
  selected section's fields on the right. Tab moves between the two,
  `j`/`k` and arrows between fields, Enter edits, Esc cancels an edit.
- **Every key in 4.2 is in it:** General (`active_provider`,
  `refresh_hours`), Player (`mpv_path`, `extra_mpv_args`), Recording
  (`recordings_dir`, `ffmpeg_path`, `record_before`, `record_after`),
  Downloads (`downloads_dir`). SPEC 4.2 gains the rule that a new key
  names its section.
- **Each field saves on confirm.** No Save button, no staged changes.
- **Wrong values are impossible or explained:**
  - Fixed choices are controls that only allow valid values:
    `refresh_hours` a 12/6 toggle, padding a stepper (0 to 60 min),
    `active_provider` a picker of the providers plus "last used".
  - Free text is checked on confirm and a refusal names the reason under
    the field, never writing the value: `Must be an absolute path`,
    `/srv/tv: not writable by you`, `mpv: not found on PATH`,
    `extra args: unmatched quote`.
  - A directory that does not exist offers `Create it`.
- **Unknown keys stay an error** (4.2). The app writes only known keys.
- Full mouse support (8.4): sections, fields, toggles and steppers are
  clickable.
- Providers and EPG are empty sections here, filled by `onboarding.md`
  and `epg-sources.md`.
- SPEC 4.2, 8.2, 8.3, 8.4 updated.

## Proposed (settle in the tickets)

- `extra_mpv_args` edited as one line, split shell-style, shown back as
  the TOML array it writes.
- Writes go to a temporary file in the same directory, renamed over the
  original with its mode kept, so a crash never leaves half a file.
- A value that fails on use despite passing the check (a directory
  removed later) is reported where it is used, as now.
