#include <assert.h>
#include <string.h>
#include <math.h>
#include "er_skate.h"
#include "libsm64.h"
#include "decomp/include/sm64.h"
#include "decomp/include/audio_defines.h"
#include "decomp/game/mario.h"
/* Production momentum/controller code, with explicit native collision seams. */
static struct SM64SurfaceCollisionData floor_surface;
static struct Object object;
static int ground_calls, air_calls, collision, fall_checks, injury, sounds, foot_cues, ground_left;
u32 set_mario_action(struct MarioState *m,u32 a,u32 arg) {m->action=a;m->actionArg=arg;return 1;}
void mario_set_forward_vel(struct MarioState *m,f32 speed) {
    float yaw=m->faceAngle[1]*3.14159265358979323846f/32768;
    m->forwardVel=speed;m->vel[0]=sinf(yaw)*speed;m->vel[2]=cosf(yaw)*speed;
}
s16 set_mario_animation(struct MarioState *m,s32 id) {(void)m;(void)id;return 0;}
void play_sound(uint32_t id,f32 *pos) {assert(pos);sounds++;if(id==SOUND_ACTION_TERRAIN_STEP)foot_cues++;}
s32 perform_ground_step(struct MarioState *m) {
    ground_calls++;if(!collision){m->pos[0]+=m->vel[0];m->pos[2]+=m->vel[2];}return ground_left ? GROUND_STEP_LEFT_GROUND : (collision ? collision : GROUND_STEP_NONE);
}
s32 perform_air_step(struct MarioState *m,u32 arg) {
    (void)arg;air_calls++;
    if(!collision){m->pos[0]+=m->vel[0];m->pos[2]+=m->vel[2];m->pos[1]+=m->vel[1];}
    m->vel[1]-=4;if(collision==AIR_STEP_LANDED)m->pos[1]=m->floorHeight;return collision;
}
s32 check_fall_damage_or_get_stuck(struct MarioState *m,u32 a) {
    fall_checks++;if(injury){m->hurtCounter=8;m->action=a;}return injury;
}
s32 lava_boost_on_wall(struct MarioState *m) {m->action=ACT_LAVA_BOOST;return 0;}
static struct MarioState mario(void) {
    struct MarioState m={0};m.health=0x880;m.action=ACT_IDLE;m.floor=&floor_surface;m.marioObj=&object;
    floor_surface.normal.x=floor_surface.normal.z=0;floor_surface.normal.y=1;return m;
}
static void mount(struct MarioState *m) {
    er_skate_configure(1);er_skate_reset();
    er_skate_input(1,0,0,0,0,0,0);assert(!er_skate_step(m));
    er_skate_input(1,1,0,0,0,0,0);assert(er_skate_step(m));assert(er_skate.mounted);
    er_skate_input(1,0,0,0,0,0,0);
}
int main(void) {
    struct MarioState m=mario(),before=m;
    er_skate_configure(0);er_skate_input(1,1,1,1,1,1,0);
    assert(!er_skate_step(&m)&&!memcmp(&m,&before,sizeof m));
    assert(!ground_calls&&!air_calls&&!sounds);
    mount(&m);
    er_skate_input(1,0,1,0,0,0,0);
    int g=ground_calls;
    float top_speed=0;
    for(int i=0;i<100;i++){assert(er_skate_step(&m));if(er_skate.speed>top_speed)top_speed=er_skate.speed;}
    assert(ground_calls-g==100);assert(!air_calls);assert(top_speed==90&&er_skate.speed<=90&&er_skate.speed>85);
    float fast=er_skate.speed;
    er_skate_configure(1);assert(er_skate.mounted&&er_skate.speed==fast); /* replay */
    er_skate_input(1,0,0,0,0,0,0);assert(er_skate_step(&m));
    assert(er_skate.speed<fast&&er_skate.speed>85); /* coast, not walk acceleration */
    er_skate.speed=20;floor_surface.normal.z=0.5f;
    er_skate_step(&m);assert(er_skate.speed>20); /* downhill */
    er_skate.speed=20;floor_surface.normal.z=-0.5f;
    er_skate_step(&m);assert(er_skate.speed<20); /* uphill */
    floor_surface.normal.z=0;
    er_skate_input(1,0,1,1,0,0,0);
    for(int i=0;i<40;i++)er_skate_step(&m);
    assert(er_skate.speed==0); /* brake wins over push, never reverse */
    er_skate.speed=10;
    er_skate_input(1,0,1,0,0,1,0);er_skate_step(&m);
    assert(m.faceAngle[1]>0&&er_skate.lean>0&&m.vel[0]>0);
    er_skate_input(1,0,0,0,0,NAN,0);assert(er_skate.steer==0);
    collision=GROUND_STEP_HIT_WALL;float z=m.pos[2];er_skate_step(&m);
    assert(er_skate.speed==0&&m.pos[2]==z);collision=0;
    er_skate_input(1,0,1,0,1,0,0);g=ground_calls;int a=air_calls;
    assert(er_skate_step(&m)&&ground_calls==g&&air_calls==a+1);
    assert(er_skate.airborne&&er_skate.trick==1&&m.vel[1]==40);
    float vy=m.vel[1];er_skate_step(&m);assert(m.vel[1]==vy-4); /* held X doesn't relaunch */
    er_skate_input(1,1,0,0,0,0,0);er_skate_step(&m);assert(er_skate.mounted); /* no air dismount */
    collision=AIR_STEP_LANDED;er_skate_step(&m);
    assert(fall_checks==1&&er_skate.mounted&&!er_skate.airborne&&!er_skate.trick);
    collision=0;er_skate_input(1,0,0,0,0,0,0);er_skate_step(&m);
    er_skate_input(1,1,0,0,0,0,0);assert(!er_skate_step(&m)&&!er_skate.mounted);
    mount(&m);er_skate_input(0,0,0,0,0,0,0);assert(!er_skate.mounted);
    er_skate_input(1,1,0,0,1,0,0);assert(!er_skate_step(&m));
    er_skate_input(1,0,0,0,0,0,0);er_skate_step(&m);
    er_skate_input(1,1,0,0,0,0,0);assert(er_skate_step(&m)&&er_skate.mounted);
    m.action=ACT_GROUND_POUND;assert(!er_skate_step(&m)&&!er_skate.mounted);
    m=mario();mount(&m);m.health=0xff;assert(!er_skate_step(&m)&&!er_skate.mounted);
    m=mario();mount(&m);er_skate_input(1,0,0,0,1,0,0);er_skate_step(&m);
    injury=1;collision=AIR_STEP_LANDED;er_skate_step(&m);
    assert(fall_checks==2&&!er_skate.mounted&&er_skate.bail_ticks==15);
    collision=injury=0;m=mario();mount(&m);er_skate_reset();assert(!er_skate.mounted);
    er_skate_input(1,1,0,0,0,0,0);assert(!er_skate_step(&m)); /* lifecycle held latch */
    m=mario();mount(&m);m.heldObj=&object;assert(!er_skate_step(&m)&&!er_skate.mounted);
    m=mario();mount(&m);m.floor=0;assert(!er_skate_step(&m)&&!er_skate.mounted);
    m=mario();mount(&m);er_skate_configure(0);before=m;
    assert(!er_skate_step(&m)&&!memcmp(&m,&before,sizeof m));
    /* Force follows the same phase used by the foot pose. Twelve small
       contact impulses preserve1.8 mean acceleration over a whole cycle. */
    m=mario();mount(&m);er_skate.speed=20;int cues=foot_cues;
    er_skate_input(1,0,1,0,0,0,0);
    for(unsigned phase=1;phase<=24;phase++) {
        float old_speed=er_skate.speed;int old_calls=ground_calls;
        assert(er_skate_step(&m)&&ground_calls==old_calls+1);
        assert(er_skate.push_phase==phase);
        float delta=er_skate.speed-old_speed;
        assert(fabsf(delta-((phase>=7&&phase<=18)?3.5f:-0.1f))<0.0001f);
    }
    assert(fabsf(er_skate.speed-60.8f)<0.001f&&foot_cues==cues+1);
    er_skate_step(&m);assert(er_skate.push_phase==1);
    er_skate_input(1,0,1,1,0,0,0);er_skate_step(&m);assert(!er_skate.push_phase);
    er_skate_input(1,0,0,0,0,0,0);er_skate_step(&m);assert(!er_skate.push_phase);
    /* A grounded request is ignored even when the same tick takes off. */
    for(unsigned requested=0;requested<=3;requested++) {
        m=mario();mount(&m);er_skate.speed=30;
        int old_air=air_calls,old_ground=ground_calls;
        er_skate_input(1,0,0,0,1,0,requested);er_skate_step(&m);
        assert(er_skate.trick==1&&!er_skate.trick_used&&er_skate.trick_ticks==1);
        assert(air_calls==old_air+1&&ground_calls==old_ground&&m.vel[1]==40);
        er_skate_step(&m);assert(er_skate.trick==1&&m.vel[1]==36);
    }
    /* Standalone airborne buttons do not change any impulse or physics step.
       Compare whole trajectories with a no-trick run, including ordinary tuck. */
    float trace_y[26],trace_z[26],trace_vy[26],trace_speed[26];
    for(unsigned selected=0;selected<=3;selected++) {
        if(selected==1)continue;
        m=mario();mount(&m);er_skate.speed=30;
        er_skate_input(1,0,0,0,1,0,0);er_skate_step(&m);
        for(int tick=0;tick<26;tick++) {
            int old_air=air_calls,old_ground=ground_calls;
            er_skate_input(1,0,0,0,0,0,tick>=2?selected:0);
            float old_vy=m.vel[1];er_skate_step(&m);
            assert(air_calls==old_air+1&&ground_calls==old_ground&&m.vel[1]==old_vy-4);
            if(!selected){trace_y[tick]=m.pos[1];trace_z[tick]=m.pos[2];trace_vy[tick]=m.vel[1];trace_speed[tick]=er_skate.speed;}
            else {
                assert(m.pos[1]==trace_y[tick]&&m.pos[2]==trace_z[tick]&&m.vel[1]==trace_vy[tick]&&er_skate.speed==trace_speed[tick]);
                if(tick>=2)assert(er_skate.trick==selected&&er_skate.trick_used);
                if(tick==2)assert(er_skate.trick_ticks==1);
            }
        }
        assert(er_skate.trick_ticks==20&&!er_skate.push_phase);
        float progress=(float)er_skate.trick_ticks/20;
        assert(isfinite(progress)&&progress==1);
        /* Releasing/repressing either button cannot start a second board trick. */
        unsigned chosen=er_skate.trick;
        er_skate_input(1,0,0,0,0,0,0);er_skate_step(&m);
        er_skate_input(1,0,0,0,0,0,chosen==2?3:2);er_skate_step(&m);
        if(selected)assert(er_skate.trick==chosen&&er_skate.trick_ticks==20);
        collision=AIR_STEP_LANDED;er_skate_step(&m);collision=0;
        assert(!er_skate.trick&&!er_skate.trick_ticks&&!er_skate.trick_used&&!er_skate.airborne);
        /* The held landing button does not buffer another trick at next takeoff. */
        er_skate_input(1,0,0,0,1,0,chosen==2?3:2);er_skate_step(&m);
        assert(er_skate.trick==1&&!er_skate.trick_used);
        er_skate_step(&m);assert(er_skate.trick==1&&!er_skate.trick_used);
        er_skate_input(1,0,0,0,0,0,0);er_skate_step(&m);
        er_skate_input(1,0,0,0,0,0,2);er_skate_step(&m);
        assert(er_skate.trick==2&&er_skate.trick_used&&er_skate.trick_ticks==1);
    }
    /* Rolling off a ledge allows a fresh airborne trick without an ollie. */
    m=mario();mount(&m);
    er_skate_input(1,0,0,0,0,0,2);er_skate_step(&m);assert(!er_skate.trick);
    ground_left=1;er_skate_step(&m);ground_left=0;
    assert(er_skate.airborne&&!er_skate.trick&&!er_skate.trick_used);
    er_skate_step(&m);assert(!er_skate.trick); /* Ground-held button stays ignored. */
    er_skate_input(1,0,0,0,0,0,0);er_skate_step(&m);
    er_skate_input(1,0,0,0,0,0,3);float old_vy=m.vel[1];er_skate_step(&m);
    assert(er_skate.trick==3&&er_skate.trick_used&&er_skate.trick_ticks==1&&m.vel[1]==old_vy-4);
    er_skate_input(0,0,0,0,0,0,2);
    assert(!er_skate.mounted&&!er_skate.trick_used&&!er_skate.trick_ticks&&!er_skate.push_phase);
    /* Suspension requires an allowed neutral observation, including across a
       fresh mount. A held request cannot sneak through loading or menus. */
    m=mario();er_skate_input(1,0,0,0,0,0,2);assert(!er_skate_step(&m));
    er_skate_input(1,1,0,0,0,0,2);assert(er_skate_step(&m));
    er_skate_input(1,0,0,0,1,0,2);er_skate_step(&m);er_skate_step(&m);
    assert(er_skate.trick==1&&!er_skate.trick_used);
    er_skate_input(1,0,0,0,0,0,0);er_skate_step(&m);
    er_skate_input(1,0,0,0,0,0,2);er_skate_step(&m);
    assert(er_skate.trick==2&&er_skate.trick_ticks==1);
    m.hurtCounter=1;assert(!er_skate_step(&m));
    assert(!er_skate.mounted&&!er_skate.trick_ticks&&!er_skate.trick&&!er_skate.trick_used);
    /* Reset and unknown inputs remain fail-closed and keep OFF parity above. */
    m=mario();mount(&m);er_skate_input(1,0,0,0,1,0,999);er_skate_step(&m);
    er_skate_step(&m);assert(er_skate.trick==1&&!er_skate.trick_used);
    er_skate_reset();assert(er_skate.trick_release&&!er_skate.trick_used);
    return 0;
}
