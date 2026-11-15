#ifndef MULTIPLAYER_OBJECTS_H
#define MULTIPLAYER_OBJECTS_H

#include "types.h"

// Numbers the current area's objects in spawn order, the same in every game with the session's
// world. Call once the area's objects have all spawned.
void mp_number_area_objects(void);

// The number of an object the area placed, or 0 for any other object.
u16 mp_object_id(struct Object *obj);

// The object the area placed that has number `id`, if it is still here. `id` may come from the
// network.
struct Object *mp_numbered_object(u32 id);

// Before Mario's update: takes the states other objects' owners sent, and removes the objects
// their owners report gone.
void mp_take_object_states(void);

// Runs the current object's update, or, for a copy of an object another game runs, mirrors the
// owner's state instead. Called for every object in place of cur_obj_update.
void mp_cur_obj_update(void);

// After Mario's update: claims the copies he attacked or picked up, so this game runs them.
void mp_claim_touched_objects(void);

// After Mario's update: reports the coins he collected, which every player in the level shares.
void mp_report_coins(void);

// After Mario's update: publishes the state of every synced object this game owns.
void mp_publish_objects(void);

// Whether `value` is a number: NaN or infinity must not reach the renderer's matrix math.
// These two are here for every file of the segment, which has no room for a copy in each.
s32 mp_is_finite(f32 value);

// `value` as an s16, or the nearest one to it.
s16 mp_saturate_s16(f32 value);

#endif
