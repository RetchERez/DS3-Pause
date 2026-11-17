# DS3 Pause

A real pause button for Dark Souls III, **offline only**.

Press **Pause/Break** (or hold **Back+Start** on a controller, then let go) and the whole game
freezes: enemies, timers, physics, music and sound. Press it again to continue.

## Install

Download **`DS3 Pause v1.0.zip`** from Releases and extract everything in it into your game
folder, next to `DarkSoulsIII.exe`:

```
...\steamapps\common\DARK SOULS III\Game\
```

The zip holds `dinput8.dll`, `DS3 Pause.exe` and a READ ME. Start DS3 the way you normally do.
DS3 Pause starts with the game, runs invisibly in the background, and closes when the game
closes.

**To uninstall,** delete those files (and `DS3 Pause.ini` / `DS3 Pause.log` if present).

**Already using another `dinput8.dll` mod?** Leave theirs in place and skip ours. Instead, either
run `DS3 Pause.exe` yourself before or after starting the game, or with **Mod Engine 2** add our
DLL to `external_dlls` in `config_darksouls3.toml` (keep `DS3 Pause.exe` in the same folder as the
DLL):

```toml
external_dlls = ["C:\\path\\to\\DS3 Pause\\dinput8.dll"]
```

## Offline only

DS3 Pause turns itself off while Dark Souls III is online. Offline DS3 opens no network ports at
all; online DS3 opens them as soon as it connects to FromSoftware's servers. DS3 Pause checks
every second, and once it sees the game with a network port open, the pause key is ignored for
the rest of that session. Restart the game offline and it works again.

To play offline: in DS3, **Settings → Network → Launch Setting → Start offline**, then restart the
game. (Or put Steam in offline mode.)

Resuming is always allowed, so a paused game can never get stuck.

## Settings (`DS3 Pause.ini`, created on first run)

| Setting | Default | Meaning |
|---|---|---|
| `Key` | `PAUSE` | Keyboard toggle: `PAUSE`, `F1`–`F12`, `A`–`Z`, `INSERT`, `HOME`, `SCROLLLOCK`, `NUMPAD0`–`9`, or a hex code like `0x13` |
| `Controller` | `true` | Use an XInput gamepad combo |
| `PadCombo` | `BACK+START` | Buttons: `A B X Y LB RB BACK START L3 R3 UP DOWN LEFT RIGHT` |
| `OnlyWhenFocused` | `true` | Pausing needs DS3 in front. Resuming always works. |
| `Beep` | `false` | Beep on pause/resume |
| `Process` | `DarkSoulsIII.exe` | The game to pause |

The controller combo fires when you **release** the buttons, so the game doesn't wake up with
them still held. The game may notice the first button press (and open a menu) before the pause
kicks in. That's harmless; change `PadCombo` if it bothers you.

## How it works

- **Loader:** DS3 loads `dinput8.dll` from its own folder first. Ours passes every DirectInput call
  straight through to Windows' real `System32\dinput8.dll` and starts `DS3 Pause.exe` in the
  background. The pause lives in that separate process because it freezes every thread of the
  game.
- **Pause:** suspends the game process with `NtSuspendProcess` (the same call as Resource
  Monitor's "Suspend process") and resumes it with `NtResumeProcess`.
- **Silence:** right before freezing, it mutes DS3 in the Windows volume mixer so no sound can keep
  looping. On resume it puts back whatever mute setting DS3 had.
- **Offline check:** reads Windows' TCP and UDP tables for the game's process (what
  `netstat -ano` shows).

It never reads or writes game memory and doesn't touch any game file or save.

## Safeguards

- **Offline only:** once the game goes online, the pause key is ignored until DS3 is restarted.
- **Focus:** pausing only works while DS3 is the active window.
- **Never stuck:** resuming always works.
- **Shutdown guard:** if Windows shuts down, restarts or logs off while the game is paused, DS3
  Pause resumes and un-mutes the game first, so it closes normally.
- **Crash recovery:** if DS3 Pause is ever force-killed while the game is paused, start it again
  (or restart the game). It un-freezes and un-mutes the game as soon as it attaches.
- **One copy:** only one DS3 Pause runs at a time.

Status messages go to `DS3 Pause.log` next to the exe.

## Known limits

- If you click or type into the frozen window for ~5 seconds, Windows may label it
  "(Not Responding)". That's cosmetic; it clears when you resume.
- Alt-tabbing out of Full Screen while paused can misbehave; resume first.

## Build

```
cargo build --release
cargo build --release --manifest-path loader/Cargo.toml
```

Outputs: `target\release\ds3pause.exe` (shipped as `DS3 Pause.exe`) and
`loader\target\release\dinput8.dll`. `package\make_zip.ps1` builds the release zip.
