#ifndef VOLUMA_H
#define VOLUMA_H
// Returns a JSON string: {"ok": ...} or {"error": "..."}. Free with voluma_free_string.
char *voluma_inspect_image(const char *path);
void voluma_free_string(char *s);
// part < 0 browses the whole image. Result JSON: {"ok": ...} or {"error": "..."}.
char *voluma_list_directory(const char *image, int part, const char *path);
// Streams a file out of the image into a NEW host file (fails if dest exists).
char *voluma_export_file(const char *image, int part, const char *fs_path, const char *dest);
#endif
