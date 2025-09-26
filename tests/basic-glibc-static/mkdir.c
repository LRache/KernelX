#include <stdio.h>
#include <sys/stat.h>

int main() {
    const char *dirname = "testdir";
    int result = mkdir(dirname, 0755);
    if (result == 0) {
        printf("Directory '%s' created successfully.\n", dirname);
        FILE *f = fopen("testdir", "r");
        if (f) {
            fclose(f);
            printf("Directory '%s' opened successfully.\n", dirname);
        } else {
            perror("Failed to open the directory");
        }
    } else {
        perror("mkdir failed");
    }
    return 0;
}
