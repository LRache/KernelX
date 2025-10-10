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

            FILE *f = fopen("testdir/file.txt", "w");
            if (f) {
                fprintf(f, "Hello, World!\n");
                fclose(f);
                printf("File 'testdir/file.txt' created and written successfully.\n");
            } else {
                perror("Failed to create file in directory");
            }
        } else {
            perror("Failed to open the directory");
        }
    } else {
        perror("mkdir failed");
    }
    return 0;
}
