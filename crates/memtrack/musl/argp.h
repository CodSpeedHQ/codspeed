/* crates/memtrack/musl/argp.h — stub for musl builds of libbpf-sys' vendored elfutils.
   Declarations only: a libelf-only build never calls into argp, but the
   elfutils sources still `#include <argp.h>`, which musl does not ship.
   If compilation complains about a missing type or macro, add it here. */
#ifndef CODSPEED_STUB_ARGP_H
#define CODSPEED_STUB_ARGP_H

#include <stdio.h>

typedef int error_t;

struct argp_option {
  const char *name;
  int key;
  const char *arg;
  int flags;
  const char *doc;
  int group;
};

struct argp_state {
  const char *name;
};

typedef error_t (*argp_parser_t)(int key, char *arg, struct argp_state *state);

struct argp {
  const struct argp_option *options;
  argp_parser_t parser;
  const char *args_doc;
  const char *doc;
  const void *children;
  void *help_filter;
  const char *argp_domain;
};

#define OPTION_ARG_OPTIONAL 0x1
#define ARGP_HELP_SEE 0x40
#define ARGP_ERR_UNKNOWN 1

int argp_help(const struct argp *argp, FILE *stream, unsigned int flags, char *name);

#endif /* CODSPEED_STUB_ARGP_H */
