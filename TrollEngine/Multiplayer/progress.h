#ifndef MULTIPLAYER_PROGRESS_H
#define MULTIPLAYER_PROGRESS_H

// Each frame in a level: publishes the current save file's progress for the bridge.
void mp_publish_progress(void);

// Each frame in a level: adds the session's progress, which the bridge wrote, to the current save
// file, and saves it if anything was new.
void mp_take_progress(void);

#endif
