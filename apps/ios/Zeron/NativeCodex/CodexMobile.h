#include <stdint.h>
uint64_t zeron_codex_open(const char *options);
int32_t zeron_codex_send(uint64_t handle, const char *message);
char *zeron_codex_poll(uint64_t handle);
void zeron_codex_free(char *message);
void zeron_codex_close(uint64_t handle);

char *zeron_git_run(const char *request);
void zeron_git_cancel(const char *command_id);
