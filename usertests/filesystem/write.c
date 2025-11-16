#include <stdio.h>
#include <fcntl.h>
#include <unistd.h>

char data[0x400 * 4];

int main() {
    int fd = open("/filesystem/a.txt", O_WRONLY);
    if (fd < 0) {
        perror("open");
        return 1;
    }

    for (unsigned int i = 0; i < sizeof(data); i++) {
        data[i] = 'A' + (i % 26);
    }
    
    write(fd, data, sizeof(data));
    write(fd, data, 1);
    
    close(fd);

    return 0;
}