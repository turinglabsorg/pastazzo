#ifndef PASTAZZO_MOBILE_H
#define PASTAZZO_MOBILE_H
#include <stddef.h>
#include <stdint.h>
char *pastazzo_mobile_call(const uint8_t *bytes, size_t length);
void pastazzo_mobile_free(char *result);
#endif
