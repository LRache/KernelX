#ifndef KERNELX_KMODULE_H
#define KERNELX_KMODULE_H

#ifdef __cplusplus
extern "C" {
#endif

long klog_info(const char *file, unsigned int line, unsigned int column, const char *fmt, ...);

#define kinfo(fmt, ...) klog_info(__FILE__, __LINE__, 0, fmt, ##__VA_ARGS__)

struct kmodule_info {
    char module_name[256];
    void *init;
    void *exit;
};

#define KERNELX_MODULE(module, init_fn, exit_fn)                                                                      \
    static const struct kmodule_info __kernelx_module_info __attribute__((used, section("kernelx.module.info"))) = { \
        .module_name = module,                                                                                        \
        .init = (void *)(init_fn),                                                                                    \
        .exit = (void *)(exit_fn),                                                                                    \
    }

#ifdef __cplusplus
}
#endif

#endif
