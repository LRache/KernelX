#include <stdio.h>
#include <sys/uio.h>
#include <unistd.h>

int main() {
    struct iovec iov[2];
    char buf1[] = "Hello, ";
    char buf2[] = "World!";
    iov[0].iov_base = buf1;
    iov[0].iov_len = sizeof(buf1) - 1; // Exclude null terminator
    iov[1].iov_base = buf2;
    iov[1].iov_len = sizeof(buf2) - 1; // Exclude null terminator
    ssize_t result = writev(1, iov, 2); // Write to standard output

    write(1, buf1, sizeof(buf1) - 1);
    write(1, buf2, sizeof(buf2) - 1);
    
    if (result < 0) {
        perror("writev");
    }

    return 0;
}
