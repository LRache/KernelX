#include <klib/klib.h>
#include <kmodule/export.h>

#include <stdarg.h>
#include <stddef.h>

extern long kmodule_log_info(const unsigned char *file, unsigned int line, unsigned int column,
                             const unsigned char *msg, unsigned long len);

static char *append_char(char *pos, char *end, char ch) {
    if (pos < end) {
        *pos++ = ch;
    }
    return pos;
}

static char *append_str(char *pos, char *end, const char *str) {
    if (str == NULL) {
        str = "(null)";
    }
    while (*str != '\0') {
        pos = append_char(pos, end, *str++);
    }
    return pos;
}

static char *append_unsigned(char *pos, char *end, unsigned long value, unsigned int base) {
    char tmp[32];
    unsigned int len = 0;

    do {
        unsigned int digit = value % base;
        tmp[len++] = digit < 10 ? (char)('0' + digit) : (char)('a' + digit - 10);
        value /= base;
    } while (value != 0 && len < sizeof(tmp));

    while (len > 0) {
        pos = append_char(pos, end, tmp[--len]);
    }
    return pos;
}

static char *append_signed(char *pos, char *end, long value) {
    if (value < 0) {
        pos = append_char(pos, end, '-');
        value = -value;
    }
    return append_unsigned(pos, end, (unsigned long)value, 10);
}

long klog_info(const char *file, unsigned int line, unsigned int column, const char *fmt, ...) {
    char buffer[256];
    char *pos = buffer;
    char *end = buffer + sizeof(buffer) - 1;
    va_list args;

    va_start(args, fmt);
    while (*fmt != '\0') {
        if (*fmt != '%') {
            pos = append_char(pos, end, *fmt++);
            continue;
        }

        fmt++;
        int long_arg = 0;
        if (*fmt == 'l') {
            long_arg = 1;
            fmt++;
        }

        switch (*fmt) {
        case '%':
            pos = append_char(pos, end, '%');
            break;
        case 'c':
            pos = append_char(pos, end, (char)va_arg(args, int));
            break;
        case 's':
            pos = append_str(pos, end, va_arg(args, const char *));
            break;
        case 'd':
        case 'i':
            pos = append_signed(pos, end, long_arg ? va_arg(args, long) : va_arg(args, int));
            break;
        case 'u':
            pos = append_unsigned(pos, end, long_arg ? va_arg(args, unsigned long) : va_arg(args, unsigned int), 10);
            break;
        case 'x':
            pos = append_unsigned(pos, end, long_arg ? va_arg(args, unsigned long) : va_arg(args, unsigned int), 16);
            break;
        case 'p':
            pos = append_str(pos, end, "0x");
            pos = append_unsigned(pos, end, (unsigned long)va_arg(args, void *), 16);
            break;
        default:
            pos = append_char(pos, end, '%');
            pos = append_char(pos, end, *fmt);
            break;
        }
        fmt++;
    }
    va_end(args);

    *pos = '\0';
    return kmodule_log_info((const unsigned char *)file, line, column, (const unsigned char *)buffer,
                            (unsigned long)(pos - buffer));
}

KMODULE_EXPORT("klog_info", klog_info);
