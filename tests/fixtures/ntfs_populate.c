/* Copies a host directory tree into an NTFS image using the real ntfs-3g library (independent of
 * VOLUMA). Usage: ntfs_populate IMAGE SRC_DIR
 * Build (see build_ntfs3g.sh): cc ntfs_populate.c -I<src>/include/ntfs-3g -L<src>/libntfs-3g/.libs -lntfs-3g */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <dirent.h>
#include <sys/stat.h>
#include <unistd.h>
#include <fcntl.h>
#include "config.h"
#include "volume.h"
#include "inode.h"
#include "dir.h"
#include "attrib.h"
#include "unistr.h"

static ntfs_volume *vol;

static int copy_tree(ntfs_inode *dir_ni, const char *src) {
    DIR *d = opendir(src);
    if (!d) { perror(src); return 1; }
    struct dirent *de;
    while ((de = readdir(d))) {
        if (!strcmp(de->d_name, ".") || !strcmp(de->d_name, "..")) continue;
        char path[4096]; snprintf(path, sizeof path, "%s/%s", src, de->d_name);
        struct stat st; if (lstat(path, &st)) { perror(path); return 1; }
        ntfschar *uname = NULL;
        int ulen = ntfs_mbstoucs(de->d_name, &uname);
        if (ulen < 0) { perror("mbstoucs"); return 1; }
        if (S_ISDIR(st.st_mode)) {
            ntfs_inode *ni = ntfs_create(dir_ni, 0, uname, ulen, S_IFDIR);
            if (!ni) { perror("create dir"); return 1; }
            if (copy_tree(ni, path)) return 1;
            ntfs_inode_close(ni);
        } else if (S_ISREG(st.st_mode)) {
            ntfs_inode *ni = ntfs_create(dir_ni, 0, uname, ulen, S_IFREG);
            if (!ni) { perror("create file"); return 1; }
            ntfs_attr *na = ntfs_attr_open(ni, AT_DATA, AT_UNNAMED, 0);
            if (!na) { perror("attr open"); return 1; }
            int fd = open(path, O_RDONLY); char *buf = malloc(1 << 20); off_t pos = 0; ssize_t n;
            while ((n = read(fd, buf, 1 << 20)) > 0) {
                if (ntfs_attr_pwrite(na, pos, n, buf) != n) { perror("pwrite"); return 1; }
                pos += n;
            }
            free(buf); close(fd);
            ntfs_attr_close(na);
            ntfs_inode_close(ni);
        }
        free(uname);
    }
    closedir(d);
    return 0;
}

int main(int argc, char **argv) {
    if (argc != 3) { fprintf(stderr, "usage: %s IMAGE SRC\n", argv[0]); return 2; }
    vol = ntfs_mount(argv[1], 0);
    if (!vol) { perror("ntfs_mount"); return 1; }
    ntfs_inode *root = ntfs_inode_open(vol, FILE_root);
    if (!root) { perror("open root"); return 1; }
    int rc = copy_tree(root, argv[2]);
    ntfs_inode_close(root);
    if (ntfs_umount(vol, 0)) { perror("umount"); rc = 1; }
    return rc;
}
