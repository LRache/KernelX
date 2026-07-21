# Suites define ALL_CASES and, for each case, CASE_SOURCES_<case> and
# CASE_COMMAND_<case>. CASES is the optional subset requested by build.py.
CASES ?= $(ALL_CASES)

UNKNOWN_CASES := $(filter-out $(ALL_CASES),$(CASES))
ifneq ($(strip $(UNKNOWN_CASES)),)
$(error unknown CASES: $(UNKNOWN_CASES))
endif

.PHONY: list-cases
list-cases:
	@$(foreach case,$(ALL_CASES),printf '%s|%s|%s\n' '$(case)' '$(CASE_SOURCES_$(case))' '$(or $(CASE_COMMAND_$(case)),-)';)
