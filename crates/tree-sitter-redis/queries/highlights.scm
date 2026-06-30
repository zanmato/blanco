; Redis command highlighting. Capture names mirror tree-sitter-sequel's
; highlights.scm so the existing gpui-component theme colors apply unchanged.

[
  (command_name)
  (container_command)
] @keyword

(subcommand) @function.call

(string) @string
(integer) @number
(float) @float
(bare_word) @variable

(comment) @comment
