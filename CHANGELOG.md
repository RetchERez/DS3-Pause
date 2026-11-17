# Changelog

## v1.0 — 2026-11-16

First release.

- **Drop-in install:** extract the zip into the DS3 `Game` folder. DS3 Pause starts with the game,
  runs in the background with no window, and closes when the game closes. It also works on its
  own or through Mod Engine 2's `external_dlls`.
- Pause and resume Dark Souls III with **Pause/Break**, or **Back+Start** (fires on release) on a
  controller. It freezes the whole game: enemies, timers, physics, music and sound.
- **Guaranteed silence:** DS3 is muted in the Windows mixer while paused, and its previous mute
  setting is restored on resume.
- **Offline only:** once DS3 opens a network connection, the pause key is ignored for the rest of
  that session. Resuming is always allowed.
- **Shutdown guard:** if Windows shuts down, restarts or logs off while paused, the game is resumed
  and un-muted first, so it closes normally instead of being killed while frozen.
- If DS3 Pause is killed mid-pause, the next launch un-freezes and un-mutes the game.
- Only one copy runs at a time.
- No Visual C++ runtime needed.
- Settings in `DS3 Pause.ini`: keys, controller combo, focus rule, and beep (off by default).
