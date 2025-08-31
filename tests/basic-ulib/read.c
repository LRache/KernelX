#include <stdio.h>

#include <unistd.h>
#include <sys/fcntl.h>

int main() {
    int fd = open("/basic-ulib/test.txt", O_RDONLY);
    if (fd < 0) {
        puts("Failed to open file");
        return 1;
    }

    char buffer[128];
    ssize_t bytes_read = read(fd, buffer, sizeof(buffer) - 1);
    if (bytes_read < 0) {
        puts("Failed to read file");
        close(fd);
        return 1;
    }

    buffer[bytes_read] = '\0'; // Null-terminate the string
    puts(buffer);

    close(fd);
    
    return 0;
}
