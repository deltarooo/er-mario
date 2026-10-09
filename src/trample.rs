//! Yoshi at full speed runs over what's in his way: a share of the enemy's health, and anything
//! short of a boss is sent flying and lies there a moment.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use eldenring::cs::{ChrIns, FieldInsHandle, WorldChrMan};
use eldenring::position::HavokPosition;
use fromsoftware_shared::FromStatic;
use glam::Vec3;

use crate::combat::{self, Combat};
use crate::log;

/// Share of its full health (%): ordinary enemies, and bosses and other strong ones
const HIT: f32 = 30.0;
const HIT_STRONG: f32 = 5.0;
/// How near his path (m, on top of the enemy's own width)
const REACH: f32 = 1.5;
/// ...and how far ahead of him its middle is (m): they go flying before they can swing
const AHEAD: f32 = 1.6;
/// The same enemy isn't hit again this soon (s); longer than it lies there, since the game
/// takes a hit on its ragdoll for a kill
const AGAIN: f32 = 5.0;
/// Pushed along for this long (s) before it goes limp: the ragdoll takes over the speed it has
const PUSHED: f32 = 0.1;
const DOWN: f32 = 4.0;
/// SOUND_ACTION_BOUNCE_OFF_OBJECT
const SOUND_BUMP: i32 = 0x0459_B081;
/// Thrown ahead at this much of his speed, and up (m/s)
const CARRY: f32 = 1.1;
const LIFT: f32 = 6.5;

struct Flung {
    handle: FieldInsHandle,
    vel: Vec3,
    since: Instant,
    limp: bool,
}

static FLUNG: Mutex<Vec<Flung>> = Mutex::new(Vec::new());
static RECENT: Mutex<Option<HashMap<u64, Instant>>> = Mutex::new(None);

/// Every frame. `charge`: where Yoshi is and how fast he's going, while he's at full speed.
/// `player`: the rider, whose blow the strong ones get.
pub fn update(combat: &mut Combat, player: &ChrIns, tick: u32, dt: f32, charge: Option<(Vec3, Vec3)>) {
    let _timed = crate::yoshi::Timed(2, Instant::now());
    let mut flung = FLUNG.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, vel)) = charge {
        let mut recent = RECENT.lock().unwrap_or_else(|e| e.into_inner());
        let recent = recent.get_or_insert_with(HashMap::new);
        recent.retain(|_, t| t.elapsed().as_secs_f32() < AGAIN);
        let ahead = at + vel.normalize_or_zero() * AHEAD;
        for (handle, key, strong) in combat::in_the_way(ahead, REACH) {
            if recent.contains_key(&key) {
                continue;
            }
            recent.insert(key, Instant::now());
            combat::impact(combat, &handle, if strong { HIT_STRONG } else { HIT }, tick);
            crate::worker::call("trample sound", |_| unsafe { crate::sm64::sm64_play_sound_global(SOUND_BUMP) });
            log(format!("trample: ran into {} at {:.1} m/s", if strong { "a big or strong enemy" } else { "an enemy" }, vel.length()));
            // the strong ones stay on their feet and get a real hit as well, which is what
            // kills them at their last point. Not the others: they go limp, and the game takes
            // a hit on a ragdoll for a kill
            if strong {
                combat.blow(player, &handle);
            } else {
                flung.push(Flung { handle, vel: vel * CARRY + Vec3::Y * LIFT, since: Instant::now(), limp: false });
            }
        }
    }
    if flung.is_empty() {
        return;
    }
    let Ok(wcm) = (unsafe { WorldChrMan::instance_mut() }) else { return };
    flung.retain_mut(|f| {
        let Some(chr) = wcm.chr_ins_by_handle_mut(&f.handle) else { return false };
        let t = f.since.elapsed().as_secs_f32();
        // (the game's own fall damage would finish it off)
        chr.modules.fall.fall_timer = 0.0;
        if !f.limp {
            if t < PUSHED {
                let p = chr.modules.physics.position;
                let next = Vec3::new(p.0, p.1, p.2) + f.vel * dt;
                chr.modules.physics.position = HavokPosition(next.x, next.y, next.z, 0.0);
                chr.modules.physics.chr_proxy_pos_update_requested = true;
                chr.modules.physics.gravity_disabled = true;
            } else {
                chr.modules.physics.gravity_disabled = false;
                if chr.chr_ctrl.ragdoll_ins == 0 {
                    return false;
                }
                chr.chr_ctrl.chr_ragdoll_state = 2;
                f.limp = true;
            }
            return true;
        }
        if t < DOWN {
            return true;
        }
        // back on its feet, unless that was the end of it
        if chr.modules.data.hp > 0 {
            chr.chr_ctrl.chr_ragdoll_state = 0;
            chr.chr_ctrl.ragdoll_revive_time = 1.0;
        }
        false
    });
}
