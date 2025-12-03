#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

#define FILE_NAME "test_mmap_shared.txt"
#define FILE_SIZE 4096

int main() {
    int fd;
    char *mapped_area;
    pid_t pid;

    // 1. Create a file and write some initial data
    fd = open(FILE_NAME, O_RDWR | O_CREAT | O_TRUNC, 0666);
    if (fd < 0) {
        perror("open failed");
        return 1;
    }

    // Extend file to FILE_SIZE
    if (ftruncate(fd, FILE_SIZE) == -1) {
        perror("ftruncate failed");
        close(fd);
        return 1;
    }

    // Write initial string at the beginning
    const char *initial_msg = "Hello, World!";
    if (write(fd, initial_msg, strlen(initial_msg)) != (ssize_t)strlen(initial_msg)) {
        perror("write failed");
        close(fd);
        return 1;
    }
    fsync(fd);

    // 2. mmap the file with MAP_SHARED
    mapped_area = mmap(NULL, FILE_SIZE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (mapped_area == MAP_FAILED) {
        perror("mmap failed");
        close(fd);
        return 1;
    }

    printf("Parent: Initial content: %s\n", mapped_area);
    fflush(stdout);

    // 3. Fork a child process
    pid = fork();

    if (pid < 0) {
        perror("fork failed");
        munmap(mapped_area, FILE_SIZE);
        close(fd);
        return 1;
    } else if (pid == 0) {
        // Child process
        printf("Child: Reading content: %s\n", mapped_area);
        fflush(stdout);

        // Modify the shared memory
        const char *child_msg = "Child was here!";
        sprintf(mapped_area, "%s", child_msg);
        printf("Child: Modified content to: %s\n", mapped_area);
        fflush(stdout);

        // Sync changes to file (optional but good practice for shared mappings)
        if (msync(mapped_area, FILE_SIZE, MS_SYNC) == -1) {
            perror("msync failed");
            exit(1);
        }

        munmap(mapped_area, FILE_SIZE);
        close(fd);
        exit(0);
    } else {
        // Parent process
        int status;
        waitpid(pid, &status, 0);

        if (WIFEXITED(status) && WEXITSTATUS(status) == 0) {
            printf("Parent: Child exited successfully.\n");
        } else {
            printf("Parent: Child failed.\n");
        }
        fflush(stdout);

        // Verify changes in memory
        printf("Parent: Content after child modification: %s\n", mapped_area);
        fflush(stdout);

        if (strcmp(mapped_area, "Child was here!") == 0) {
            printf("Parent: SUCCESS! Memory reflects changes.\n");
        } else {
            printf("Parent: FAILURE! Memory does not reflect changes.\n");
        }
        fflush(stdout);

        munmap(mapped_area, FILE_SIZE);
        close(fd);
        
        // Verify changes in file
        char buffer[100];
        fd = open(FILE_NAME, O_RDONLY);
        if (fd < 0) {
            perror("open for verification failed");
            return 1;
        }
        ssize_t bytes_read = read(fd, buffer, sizeof(buffer) - 1);
        if (bytes_read >= 0) {
            buffer[bytes_read] = '\0';
            printf("Parent: File content verification: %s\n", buffer);
            if (strncmp(buffer, "Child was here!", 15) == 0) {
                 printf("Parent: SUCCESS! File reflects changes.\n");
            } else {
                 printf("Parent: FAILURE! File does not reflect changes.\n");
            }
        } else {
            perror("read failed");
        }
        fflush(stdout);
        
        close(fd);
        unlink(FILE_NAME);
    }

    return 0;
}
