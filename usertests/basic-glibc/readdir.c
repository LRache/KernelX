#include <stdio.h>
#include <stdlib.h>
#include <dirent.h>
#include <string.h>

void list_directory_contents(const char *dir_path) {
    DIR *dirp;
    struct dirent *entry;

    // 1. 打开目录流
    dirp = opendir(dir_path);
    if (dirp == NULL) {
        perror("opendir error"); // 如果打开失败，打印错误信息
        return;
    }

    while ((entry = readdir(dirp)) != NULL) {
        printf("%8lu  ", entry->d_ino);
            
        char d_type = entry->d_type;
        printf("%-10s ", (d_type == DT_REG) ?  "regular" :
                         (d_type == DT_DIR) ?  "directory" :
                         (d_type == DT_FIFO) ? "FIFO" :
                         (d_type == DT_SOCK) ? "socket" :
                         (d_type == DT_LNK) ?  "symlink" :
                         (d_type == DT_BLK) ?  "block dev" :
                         (d_type == DT_CHR) ?  "char dev" : "???");
        printf("%4d %10jd  %s\n", entry->d_reclen, entry->d_off, entry->d_name);
    }

    if (closedir(dirp) == -1) {
        perror("closedir error");
    }
}

int main(int argc, char *argv[]) {
    char *dir_path;
    if (argc != 2) {
        dir_path = ".";
    } else {
        dir_path = argv[1];
    }

    list_directory_contents(dir_path);

    return EXIT_SUCCESS;
}