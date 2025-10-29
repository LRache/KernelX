#include <stdio.h>
#include <unistd.h>

int main() {
    char *ptr = sbrk(0);
    sbrk(1024);
    ptr += 32;
    *ptr = 124;
    
    puts("brk test passed");
    
    return 0;
}
