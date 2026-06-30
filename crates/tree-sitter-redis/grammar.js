/**
 * @file Redis grammar for tree-sitter
 * @author Andreas Johansson z@zanmato.se
 * @license MIT
 */

/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

const { COMMANDS, CONTAINERS, SUBCOMMANDS } = require("./grammar/keywords.js");

// Redis commands are case-insensitive. Turn a literal like "CONFIG" into a
// case-insensitive regex source ("[Cc][Oo][Nn][Ff][Ii][Gg]"), escaping the
// non-letter characters that appear in module command names (e.g. "BF.ADD").
function caseInsensitive(word) {
  return word
    .split("")
    .map((char) => {
      if (/[a-z]/i.test(char)) {
        return `[${char.toLowerCase()}${char.toUpperCase()}]`;
      }
      return char.replace(/[.*+?^${}()|[\]\\-]/g, "\\$&");
    })
    .join("");
}

// One token rule matching any keyword in `words`, case-insensitively. `prec`
// keeps the keyword from being out-prioritized by the generic `bare_word`
// token when both match the same span.
function keywordToken(words, precedence) {
  return token(prec(precedence, new RegExp(words.map(caseInsensitive).join("|"))));
}

module.exports = grammar({
  name: "redis",

  // Only spaces and tabs are insignificant. Newlines separate commands, so they
  // must stay out of `extras`.
  extras: () => [/[ \t]/],

  rules: {
    // A buffer is a sequence of command lines. Newlines are the only command
    // separator, so they are required between commands (blank lines allowed).
    source_file: ($) =>
      seq(
        repeat($._newline),
        optional(
          seq(
            $._line,
            repeat(seq(repeat1($._newline), $._line)),
            repeat($._newline),
          ),
        ),
      ),

    _newline: () => /\r?\n/,

    _line: ($) => choice($.command, $.comment),

    // Redis has no native comment syntax; `#` to end-of-line is our own
    // config-style convention (see SqlView::line_comment_prefix), used for the
    // result annotations the editor appends after each run. It is only a comment
    // at the start of a line: in argument position `#` stays a bare_word so that
    // forms like `BITFIELD key GET u8 #0` keep highlighting correctly.
    comment: () => token(prec(1, /#[^\r\n]*/)),

    command: ($) =>
      choice(
        // Container commands (CONFIG, CLIENT, ...) take an optional subcommand.
        seq($.container_command, optional($.subcommand), repeat($._argument)),
        // Known commands matched against the live command list.
        seq($.command_name, repeat($._argument)),
        // Anything else: still parse the line so highlighting degrades cleanly
        // for unknown/typo'd commands and module commands we don't know about.
        seq($.unknown_command, repeat($._argument)),
      ),

    command_name: () => keywordToken(COMMANDS, 2),
    container_command: () => keywordToken(CONTAINERS, 3),
    subcommand: () => keywordToken(SUBCOMMANDS, 2),

    // First bare token on a line when it is not a recognized command.
    unknown_command: () => token(prec(0, /[^\s'"]+/)),

    _argument: ($) => choice($.string, $.float, $.integer, $.bare_word),

    // Mirror crates/redis/src/command.rs::tokenize: double quotes process `\`
    // escapes, single quotes are literal.
    string: () =>
      choice(
        token(seq('"', repeat(choice(/[^"\\]/, seq("\\", /./))), '"')),
        token(seq("'", /[^']*/, "'")),
      ),

    integer: () => token(prec(1, /[-+]?\d+/)),
    float: () => token(prec(1, /[-+]?\d+\.\d+([eE][-+]?\d+)?/)),

    bare_word: () => token(prec(0, /[^\s'"]+/)),
  },
});
