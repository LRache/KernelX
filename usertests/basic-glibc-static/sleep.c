
#include <stdio.h>
#include <time.h>

int main() {
    printf("Sleeping for 2 seconds...\n");
    fflush(stdout);
    
    struct timespec req = {2, 0}; // 2 seconds, 0 nanoseconds
    int r = nanosleep(&req, NULL);
    if (r != 0) {
        perror("sleep");
        return 1;
    }
    
    printf("Awake!\n");
    
    return 0;
}
