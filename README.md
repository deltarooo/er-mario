# ER Mario

Play Elden Ring as Mario, with Super Mario 64's real movement. Triple jumps, wall kicks, long
jumps, ground pounds, punches and kicks that hurt enemies, Bowser's tail swing on staggered
bosses, Bob-omb style enemy throws, SM64's health meter, coins and Lakitu's camera.

Work in progress.

You need your own Super Mario 64 ROM (US version): Mario's textures, sounds, animations and the
menu icons come from it, built on your PC the first time you play. Like other
[libsm64](https://github.com/libsm64/libsm64) projects, the mod itself includes Mario's 3D
model (his mesh) from the public [SM64 decompilation](https://github.com/n64decomp/sm64).

## Getting started

You need Elden Ring on Steam and a Super Mario 64 ROM (US version).

1. Install **me3**, the mod loader: [me3.help](https://me3.help)
2. Download **ER-Mario-x.y.z.zip** from the
   [latest release](https://github.com/deltarooo/er-mario/releases/latest) (under Assets) and
   unzip it somewhere you can write to, like your Documents folder.
3. Put your Super Mario 64 ROM into the **ER-Mario** folder.
4. Start the game by double-clicking **er-mario.me3**.
5. The first time, the mod sets itself up from your ROM: a box on the title screen shows the
   progress (a few seconds). When it says **Setup complete**, press any button to close the
   game, then start it again with **er-mario.me3**. That's only needed once.

**ER MARIO** and the version in the title screen's bottom left corner
show the mod is loaded. Start a new character, the mod uses its own save file.

The game will let you know at the bottom left corner when a new update is available.

Tested with an Xbox One controller, a PS5 controller (through Steam) and keyboard and mouse.
The mod reads controllers the way Xbox pads report them: PlayStation, Switch and other pads
work through Steam Input, which Steam turns on for them by default.

**LEVELING AND ITEMS DO NOT AFFECT ANYTHING**:
- Every hit takes a fixed share of the enemy's max health (25% for a punch, 67% for a ground pound, a twentieth of that on bosses). Strength, Dexterity and weapons play no part. The small real hit the   mod fires for the final blow is a flat 10 damage with stat scaling switched off.
- Mario's health is always the 8 SM64 wedges, whatever your Vigor.
- Stagger depends on the boss's poise, not on your stats.
- Stamina, FP, equip load are not used at all.

**Stay offline.** me3 starts the game offline with anti-cheat off. Never play this mod online.

**FOR STREAMERS:** You need to use window capture or display capture for the HUD to appear properly.

## Known issues

- While alot of them should be fixed, there is still a chance that you might clip through some elevators. Let me know which ones and I will fix them ASAP.
- Cutscenes show a crumpled Mario with the Tarnished's head.
- Some big bosses' ragdolls go wild after a throw; the mod stops them early.
- Mario's shadow can flicker or drop out from some camera angles in sunlight and moonlight.
- With a controller, Mario can sometimes keep flicking between two directions while you
  move, as if two sticks were steering him. Restart the game and it's gone. It seems to happen
  when the controller connects or reconnects while the game is already running.

## Discord

Questions, clips and news about the mod: [Delta's shenanigans](https://discord.gg/6EvmBwXp4u).

## Reporting problems

Open an [issue](https://github.com/deltarooo/er-mario/issues) and say what happened and
when. Attach both files from the **logs** folder inside your ER-Mario folder:
`er_mario.log` (the last session) and `er_mario.prev.log` (the one before). If the game didn't
start at all, me3's own log helps too: paste `%LOCALAPPDATA%\garyttierney\me3\data\logs` into
the Explorer address bar, open the **er-mario** folder and attach the newest file.

The logs contain your mod folder's path, which can include your Windows user name; feel free
to blank it out.

## How it works

- A Rust DLL loaded by [me3](https://me3.help), offline only (Easy Anti-Cheat off, separate save).
- [libsm64](https://github.com/libsm64/libsm64) runs Mario (SM64's own physics and animation),
  fed with Elden Ring's live Havok collision.
- The Tarnished stays in the game underneath and follows Mario, so doors, graces, menus, deaths
  and saves keep working.
- The `libsm64` folder is libsm64 with a few patches for the mod (ladder climbing, carrying,
  dive grabs, head turning, model part export), compiled into the DLL.
- At launch and every 5 minutes after, the mod asks GitHub for the latest release's version. The
  title screen, and a small note in the bottom left corner in game, say when there's a newer one.
  Nothing is downloaded or installed automatically. `update_check = off` in er_mario.ini turns the
  check off.

## Building

Windows, with [Rust](https://rustup.rs), Visual Studio Build Tools (C++),
[LLVM](https://github.com/llvm/llvm-project/releases) (clang-cl compiles libsm64's C code) and
Python. Cargo fetches fromsoftware-rs itself (pinned to a commit in `Cargo.toml`):

```
git clone https://github.com/deltarooo/er-mario
cd er-mario/libsm64
python import-mario-geo.py
cd ..
.\build.ps1
```

`import-mario-geo.py` (libsm64's own setup script) downloads Mario's model code, two files, from
the SM64 decompilation once and strips their texture data (textures come from the player's ROM).
The model code isn't part of this repository; it gets compiled into the DLL.

`build.ps1 -Dist <ER-Mario folder>` also copies the DLL into an ER-Mario folder (it renames
the old DLL first, so it works while the game is running; the next start loads the new one).
The release zip is that folder without the generated `package` and `logs` folders.




## License

[MIT](LICENSE). This covers the mod's own code only, not Super Mario 64's or Elden Ring's
content: Mario's textures, sounds and animations come from the player's ROM, his mesh from the
SM64 decompilation (see above), and nothing of FromSoftware's is included. The `libsm64` folder
keeps libsm64's own license, CC0 ([libsm64/LICENSE.md](libsm64/LICENSE.md)).

## Credits

- [libsm64](https://github.com/libsm64/libsm64) (CC0) by the libsm64 contributors, built on the
  [Super Mario 64 decompilation](https://github.com/n64decomp/sm64) by the n64decomp team
- [fromsoftware-rs](https://github.com/vswarte/fromsoftware-rs) (MIT / Apache-2.0) by Vincent
  Swarte and contributors
- [me3](https://me3.help) by the me3 team
- [hudhook](https://github.com/veeenu/hudhook) (MIT) by veeenu, for the overlay HUD
- Item and inventory function patterns: The Grand Archives' Elden Ring cheat table

Super Mario 64 and Mario are Nintendo's. Elden Ring is FromSoftware's. This is a free fan mod,
not affiliated with either.
