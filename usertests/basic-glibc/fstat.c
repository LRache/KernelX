#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <fcntl.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <string.h>

#define TEST_FILE "test_fstat_file.txt"
#define TEST_CONTENT "Hello, fstat!"

int main() {
    int fd;
    struct stat st;
    ssize_t bytes_written;

    // 1. Create and open a new file
    printf("Creating file: %s\n", TEST_FILE);
    fd = open(TEST_FILE, O_CREAT | O_RDWR | O_TRUNC, 0644);
    if (fd < 0) {
        perror("open failed");
        return 1;
    }

    // 2. Write content to the file
    printf("Writing content: \"%s\"\n", TEST_CONTENT);
    bytes_written = write(fd, TEST_CONTENT, strlen(TEST_CONTENT));
    if (bytes_written < 0) {
        perror("write failed");
        close(fd);
        unlink(TEST_FILE);
        return 1;
    }

    // 3. Call fstat on the file descriptor
    if (fstat(fd, &st) < 0) {
        perror("fstat failed");
        close(fd);
        unlink(TEST_FILE);
        return 1;
    }

    // 4. Output fstat results
    printf("\n--- fstat results ---\n");
    printf("File Descriptor: %d\n", fd);
    printf("Size: %ld bytes\n", st.st_size);
    printf("Inode: %ld\n", st.st_ino);
    printf("Mode: %o\n", st.st_mode);
    printf("Nlink: %ld\n", st.st_nlink);
    printf("UID: %d\n", st.st_uid);
    printf("GID: %d\n", st.st_gid);
    
    // Verify size
    if (st.st_size == strlen(TEST_CONTENT)) {
        printf("\nSUCCESS: File size matches written content length.\n");
    } else {
        printf("\nFAILURE: File size mismatch. Expected %ld, got %ld.\n", strlen(TEST_CONTENT), st.st_size);
    }

    // Verify mode (basic check for regular file)
    if (S_ISREG(st.st_mode)) {
        printf("SUCCESS: File is a regular file.\n");
    } else {
        printf("FAILURE: File is not reported as a regular file.\n");
    }

    // 5. Cleanup
    close(fd);
    unlink(TEST_FILE);
    printf("Cleaned up %s\n", TEST_FILE);

    return 0;
}
