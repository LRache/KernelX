#include <stdio.h>
#include <string.h>
#include <unistd.h>
#include <sys/fcntl.h>

int main() {
    int fd = open("/basic-ulib/test-write.txt", 0, 0644);
    if (fd < 0) {
        puts("Failed to open file for writing");
        return 1;
    }

    const char *msg = "Hello, World!";
    ssize_t bytes_written = write(fd, msg, strlen(msg));
    if (bytes_written < 0) {
        puts("Failed to write to file");
        close(fd);
        return 1;
    }

    close(fd);

    fd = open("/basic-ulib/test-write.txt", 0);
    if (fd < 0) {
        puts("Failed to reopen file for reading");
        return 1;
    }

    char buffer[128];
    ssize_t bytes_read = read(fd, buffer, sizeof(buffer) - 1);
    if (bytes_read < 0) {
        puts("Failed to read from file");
        close(fd);
        return 1;
    }

    buffer[bytes_read] = '\0'; // Null-terminate the string
    puts(buffer);

    close(fd);

    return 0;
}
