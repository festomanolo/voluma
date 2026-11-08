#ifndef VOLUMA_H
#define VOLUMA_H
// Returns a JSON string: {"ok": ...} or {"error": "..."}. Free with voluma_free_string.
char *voluma_inspect_image(const char *path);
void voluma_free_string(char *s);
#endif
