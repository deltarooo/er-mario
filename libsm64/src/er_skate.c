/* Original momentum skating controller. No SK8/Skate 3 source or assets. */
#include "er_skate.h"
#include "decomp/include/sm64.h"
#include "decomp/include/mario_animation_ids.h"
#include "decomp/include/audio_defines.h"
#include "play_sound.h"
#include "decomp/game/mario.h"
#include "decomp/game/mario_step.h"
#include <math.h>
#include <string.h>

struct ERSkate er_skate;
s32 check_fall_damage_or_get_stuck(struct MarioState *m, u32 action);
s32 lava_boost_on_wall(struct MarioState *m);

static void dismount(void) {
    er_skate.mounted = er_skate.airborne = er_skate.trick = 0;
    er_skate.push_phase = er_skate.trick_ticks = 0;
    er_skate.trick_used = 0;
    er_skate.trick_release = 1;
    er_skate.speed = er_skate.lean = 0;
}
void er_skate_reset(void) {
    uint32_t enabled = er_skate.enabled;
    memset(&er_skate, 0, sizeof er_skate);
    er_skate.enabled = enabled;
    /* Loading while a button is held must not mount or ollie on return. */
    er_skate.toggle_release = er_skate.ollie_release = er_skate.trick_release = 1;
}
void er_skate_configure(uint32_t enabled) {
    enabled = !!enabled;
    if (er_skate.enabled == enabled) return;
    er_skate.enabled = enabled;
    er_skate_reset();
}
void er_skate_input(uint32_t allowed, uint32_t toggle, uint32_t push,
                    uint32_t brake, uint32_t ollie, float steer, uint32_t trick) {
    er_skate.allowed = !!allowed;
    er_skate.toggle = !!toggle; er_skate.push = !!push;
    er_skate.brake = !!brake; er_skate.ollie = !!ollie;
    er_skate.trick_request = trick == 2 || trick == 3 ? trick : 0;
    er_skate.steer = isfinite(steer) ? fmaxf(-1, fminf(1, steer)) : 0;
    if (!allowed) {
        dismount();
        er_skate.toggle_release = er_skate.ollie_release = 1;
    }
}
static int safe(struct MarioState *m) {
    if (!m->floor || m->health < 0x100 || m->heldObj || m->hurtCounter
        || m->invincTimer > 0 || m->quicksandDepth > 0) return 0;
    switch (m->action) {
        case ACT_IDLE: case ACT_WALKING: case ACT_BRAKING: case ACT_DECELERATING:
        case ACT_FREEFALL: case ACT_JUMP: case ACT_JUMP_LAND: case ACT_FREEFALL_LAND:
            return 1;
        default: return 0;
    }
}
int er_skate_step(struct MarioState *m) {
    if (!er_skate.enabled) return 0; /* OFF never touches Mario. */
    int allowed = er_skate.allowed && safe(m);
    int ground = !(m->action & ACT_FLAG_AIR) && m->pos[1] <= m->floorHeight + 5;
    if (!allowed) {
        dismount(); er_skate.toggle_release = er_skate.ollie_release = 1;
        er_skate.previous_toggle = er_skate.toggle;
        er_skate.previous_ollie = er_skate.ollie;
        er_skate.previous_trick = er_skate.trick_request;
        return 0;
    }
    if (!er_skate.toggle) er_skate.toggle_release = 0;
    if (!er_skate.ollie) er_skate.ollie_release = 0;
    if (!er_skate.trick_request) er_skate.trick_release = 0;
    int toggle = er_skate.toggle && !er_skate.previous_toggle && !er_skate.toggle_release;
    int ollie = er_skate.ollie && !er_skate.previous_ollie && !er_skate.ollie_release;
    int trick = er_skate.trick_request && !er_skate.previous_trick && !er_skate.trick_release;
    er_skate.previous_toggle = er_skate.toggle;
    er_skate.previous_ollie = er_skate.ollie;
    er_skate.previous_trick = er_skate.trick_request;
    if (er_skate.bail_ticks) er_skate.bail_ticks--;
    if (toggle && ground && !er_skate.bail_ticks) {
        if (er_skate.mounted) { dismount(); return 0; }
        er_skate.mounted = 1;
        er_skate.speed = fmaxf(0, fminf(90, m->forwardVel));
    }
    if (!er_skate.mounted) return 0;
    /* Consume edges even on the ground: held buttons cannot buffer a trick
       for takeoff. One fresh airborne edge changes presentation only. */
    if ((m->action & ACT_FLAG_AIR) && trick && !er_skate.trick_used) {
        er_skate.trick = er_skate.trick_request;
        er_skate.trick_ticks = 0;
        er_skate.trick_used = 1;
    }
    int turn = (int)(er_skate.steer * (ground ? 600.0f : 260.0f));
    m->faceAngle[1] += turn;
    er_skate.lean += (er_skate.steer - er_skate.lean) * 0.25f;
    /* Twelve contact ticks in a 24-tick push cycle preserve the old mean
       acceleration. Force is distributed across the foot plant, never a
       single large impulse; air, braking and release immediately stop it. */
    if (ground && er_skate.push && !er_skate.brake && !ollie)
        er_skate.push_phase = er_skate.push_phase % 24 + 1;
    else er_skate.push_phase = 0;
    if (ground) {
        float yaw = m->faceAngle[1] * (3.14159265358979323846f / 32768.0f);
        float slope = m->floor->normal.x * sinf(yaw) + m->floor->normal.z * cosf(yaw);
        er_skate.speed += slope * 2.8f;
        if (er_skate.push_phase >= 7 && er_skate.push_phase <= 18)
            er_skate.speed += 3.6f;
        if (er_skate.push_phase == 7)
            play_sound(SOUND_ACTION_TERRAIN_STEP + m->terrainSoundAddend, m->marioObj->header.gfx.cameraToObject);
        er_skate.speed -= er_skate.brake ? 3.5f : 0.10f + fabsf(er_skate.steer) * 0.16f;
        er_skate.speed = fmaxf(0, fminf(90, er_skate.speed));
        if (ollie) {
            float speed = er_skate.speed;
            set_mario_action(m, ACT_FREEFALL, 0);
            m->vel[1] = 44; m->peakHeight = m->pos[1];
            er_skate.speed = speed;
            er_skate.trick = 1;
            er_skate.trick_ticks = 0; ground = 0;
            play_sound(SOUND_ACTION_TERRAIN_JUMP + m->terrainSoundAddend, m->marioObj->header.gfx.cameraToObject);
        }
    }
    /* Set the action only on transitions: walking initialization otherwise
       resets momentum and repeatedly initializes the animation. */
    if (ground && m->action != ACT_WALKING) set_mario_action(m, ACT_WALKING, 0);
    if (!ground && m->action != ACT_FREEFALL) set_mario_action(m, ACT_FREEFALL, 0);
    mario_set_forward_vel(m, er_skate.speed);
    set_mario_animation(m, ground ? MARIO_ANIM_RIDING_SHELL : MARIO_ANIM_JUMP_RIDING_SHELL);
    if (ground && er_skate.speed > 2)
        play_sound(SOUND_MOVING_TERRAIN_RIDING_SHELL + m->terrainSoundAddend, m->marioObj->header.gfx.cameraToObject);
    if (ground) {
        int result = perform_ground_step(m);
        if (result == GROUND_STEP_LEFT_GROUND) {
            set_mario_action(m, ACT_FREEFALL, 0);
            m->peakHeight = m->pos[1]; ground = 0;
        } else if (result == GROUND_STEP_HIT_WALL) {
            er_skate.push_phase = 0;
            er_skate.speed = 0; mario_set_forward_vel(m, 0);
        }
    } else {
        if (m->pos[1] > m->peakHeight) m->peakHeight = m->pos[1];
        int result = perform_air_step(m, 0);
        if (result == AIR_STEP_LANDED) {
            /* Keep native fall injury/stuck-ground behavior; a board is not
               a fall-damage immunity upgrade. */
            int injury = check_fall_damage_or_get_stuck(m, ACT_HARD_BACKWARD_GROUND_KB);
            if (injury || m->hurtCounter) { dismount(); er_skate.bail_ticks = 15; }
            else {
                set_mario_action(m, ACT_WALKING, 0);
                er_skate.trick = er_skate.trick_ticks = 0; ground = 1;
                er_skate.trick_used = 0;
                play_sound(SOUND_ACTION_TERRAIN_LANDING + m->terrainSoundAddend, m->marioObj->header.gfx.cameraToObject);
            }
        } else if (result == AIR_STEP_HIT_WALL) {
            er_skate.speed = 0; mario_set_forward_vel(m, 0);
        } else if (result == AIR_STEP_HIT_LAVA_WALL) {
            lava_boost_on_wall(m); dismount(); er_skate.bail_ticks = 15;
        }
    }
    er_skate.airborne = er_skate.mounted && !ground;
    if (er_skate.airborne) {
        er_skate.push_phase = 0;
        if (er_skate.trick && er_skate.trick_ticks < 20) er_skate.trick_ticks++;
    }
    return 1;
}
