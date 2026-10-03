#ifndef ER_SKATE_H
#define ER_SKATE_H
#include <stdint.h>
struct MarioState;
struct ERSkate {
    uint32_t enabled, mounted, allowed, airborne, trick, bail_ticks;
    uint32_t toggle, push, brake, ollie, previous_toggle, previous_ollie;
    uint32_t toggle_release, ollie_release;
    /* Push phase: 0 idle, 1..24 cycle; trick ticks saturate at20 until landing. */
    uint32_t trick_request, push_phase, trick_ticks;
    uint32_t previous_trick, trick_release, trick_used;
    float steer, speed, lean;
};
extern struct ERSkate er_skate;
void er_skate_configure(uint32_t enabled);
void er_skate_reset(void);
void er_skate_input(uint32_t allowed, uint32_t toggle, uint32_t push,
                    uint32_t brake, uint32_t ollie, float steer, uint32_t trick);
/* Worker-confined. Returns 1 only when exactly one native collision step ran. */
int er_skate_step(struct MarioState *m);
#endif
