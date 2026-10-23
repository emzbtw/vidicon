# 03: Noctalia template, docs and screenshots

Status: done. Blocked by: 02.

## Goal

A Noctalia user registers Vidicon's template once, and Vidicon follows
their scheme and wallpaper from then on.

## Context

`docs/milestones/themes.md` (the user template shape, the proposed
token mapping). Noctalia 5.3: `noctalia theme --list-templates`,
`noctalia theme <image> --dark -r <in>:<out>` renders a template
without touching the running shell; `noctalia theme <image> --dark`
prints every token. The built-in templates under Noctalia's
`share/noctalia/assets/templates/` show the placeholder syntax.

## Scope

- The template in the repo (proposed `contrib/noctalia/vidicon.toml`),
  reading `.dark` variants, mapping Noctalia's tokens onto the base
  tokens from 01. Settle the mapping, `star`/`ok`/`warn` and
  `logo-backing` here.
- Contrast check: render the template against at least four images of
  different hue and brightness (invented or freely licensed, not the
  user's wallpapers in the repo) and look at every view. A token that
  goes unreadable gets a different source token or a fixed value.
- README and site (`site/`): a Themes section with the
  `[theme.templates.user.vidicon]` lines and the `theme` setting.
- `docs/screenshots/themes.png`: the main screen under two Noctalia
  palettes, against the demo provider.
- Milestone review: `code-review` since the commit that planned it.

## Acceptance

- [x] With the template registered in a throwaway `XDG_CONFIG_HOME`,
  `--list-templates` lists it and a render writes a file Vidicon loads
  with no fallback note. (2026-10-23: registered as the README says,
  from `~/.config/vidicon/noctalia-template.toml`; listed under User
  templates; rendered by `noctalia theme <image> --dark -c`; the Theme
  field showed no note, and `No Noctalia palette at <path>` with the
  file removed.)
- [x] A wallpaper change in the running Noctalia re-themes a running
  Vidicon within about a second (checked by the user, on their
  desktop, 2026-10-23).
- [x] Contrast checked against the four renders; screenshots taken.
  (50 renders, numbers in `docs/milestones/themes.md`.)
- [x] Milestone review findings fixed. (Theme field no longer rewrites
  `config.toml` when unchanged; test helper and template header tidied.
  Left: the six-place token list in `theme.rs` (15 tokens, a macro is
  not worth it), `theme::Palette` sharing the Slint global's name, and
  `Screen`'s self-`Weak`, which the Settings save path needs to restart
  the timer from `&mut self`.)

## Out of scope

Light themes, built-in palettes, submitting to Noctalia's community
catalog.
