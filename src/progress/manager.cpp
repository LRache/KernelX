#include <kernelx/progress/manager.hpp>

using namespace kernelx;

etl::queue<progress::PCB *, 10> progress::manager::readyQueue;

void progress::manager::load() {
    extern char progress_start[];
    extern char progress_end[];

    
}
