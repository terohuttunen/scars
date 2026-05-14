/* Hosted-target wrapper for defmt's .defmt info section.
 *
 * defmt.x by itself, when passed via `-T`, replaces the linker's default
 * script. That works for embedded targets that ship their own complete
 * linker script, but on a hosted Linux PIE binary it produces a malformed
 * ELF (VirtAddr 0 for the INTERP segment, dyn loader refuses to start).
 *
 * Adding an INSERT directive switches ld/lld into "augment the default
 * script" mode, which is what we want here. The SECTIONS body mirrors
 * defmt.x exactly; keep them in sync.
 */
SECTIONS
{
  .defmt 1 (INFO) :
  {
    . = 1;
    *(.defmt.prim.*);
    __DEFMT_MARKER_TRACE_START = .;
    *(.defmt.trace.*);
    __DEFMT_MARKER_TRACE_END = .;
    __DEFMT_MARKER_DEBUG_START = .;
    *(.defmt.debug.*);
    __DEFMT_MARKER_DEBUG_END = .;
    __DEFMT_MARKER_INFO_START = .;
    *(.defmt.info.*);
    __DEFMT_MARKER_INFO_END = .;
    __DEFMT_MARKER_WARN_START = .;
    *(.defmt.warn.*);
    __DEFMT_MARKER_WARN_END = .;
    __DEFMT_MARKER_ERROR_START = .;
    *(.defmt.error.*);
    __DEFMT_MARKER_ERROR_END = .;
    *(.defmt.*);
    __DEFMT_MARKER_END = .;
    KEEP(*(.defmt.end .defmt.end.*));
  }
}
INSERT BEFORE .comment;

EXTERN(_defmt_acquire);
EXTERN(_defmt_release);
EXTERN(__defmt_default_timestamp);
EXTERN(__DEFMT_MARKER_TIMESTAMP_WAS_DEFINED);
PROVIDE(_defmt_timestamp = __defmt_default_timestamp);
PROVIDE(_defmt_panic = __defmt_default_panic);

ASSERT(__DEFMT_MARKER_END < 65534, ".defmt section cannot contain more than 65534 interned strings");
