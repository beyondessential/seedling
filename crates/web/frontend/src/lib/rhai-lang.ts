import { StreamLanguage } from "@codemirror/language";

const KEYWORDS = new Set([
  "fn", "private", "let", "const", "if", "else", "switch", "case", "default",
  "while", "loop", "do", "until", "for", "in", "return", "break", "continue",
  "throw", "try", "catch", "import", "export", "as", "is", "shared", "type",
]);

const ATOMS = new Set(["true", "false", "unit"]);

const BUILTINS = new Set([
  "print", "debug", "type_of", "len", "range",
  "push", "pop", "insert", "remove", "clear", "pad",
  "contains", "keys", "values", "entries",
  "to_string", "to_debug", "to_int", "to_float", "to_bool",
  "parse_int", "parse_float",
  "abs", "sqrt", "floor", "ceiling", "round", "sin", "cos", "tan",
  "min", "max", "clamp",
  "some", "none",
]);

interface State {
  blockCommentDepth: number;
  inString: string | null; // the closing delimiter
}

export const rhaiLanguage = StreamLanguage.define<State>({
  name: "rhai",

  startState: () => ({ blockCommentDepth: 0, inString: null }),

  copyState: (s) => ({ ...s }),

  token(stream, state) {
    // Continuation of a block comment
    if (state.blockCommentDepth > 0) {
      if (stream.match("/*")) {
        state.blockCommentDepth++;
        return "comment";
      }
      if (stream.match("*/")) {
        state.blockCommentDepth--;
        return "comment";
      }
      stream.next();
      return "comment";
    }

    // Continuation of a string
    if (state.inString) {
      if (stream.eol()) {
        // unterminated — reset; highlighted as error via "string" staying open
        state.inString = null;
        return "string";
      }
      const ch = stream.next();
      if (ch === "\\" && state.inString !== "`") {
        stream.next(); // skip escape
      } else if (ch === state.inString) {
        state.inString = null;
      }
      return "string";
    }

    if (stream.eatSpace()) return null;

    // Line comment
    if (stream.match("//")) {
      stream.skipToEnd();
      return "comment";
    }

    // Block comment (Rhai supports nesting)
    if (stream.match("/*")) {
      state.blockCommentDepth = 1;
      return "comment";
    }

    // Strings: ", ', `
    if (stream.match('"') || stream.match("'") || stream.match("`")) {
      state.inString = stream.current();
      return "string";
    }

    // Numbers
    if (stream.match(/^0x[0-9a-fA-F][0-9a-fA-F_]*/)) return "number";
    if (stream.match(/^0o[0-7][0-7_]*/)) return "number";
    if (stream.match(/^0b[01][01_]*/)) return "number";
    if (stream.match(/^[0-9][0-9_]*(\.[0-9][0-9_]*)?(e[+-]?[0-9]+)?/)) return "number";

    // Identifiers / keywords
    if (stream.match(/^[a-zA-Z_][a-zA-Z0-9_]*/)) {
      const word = stream.current();
      if (KEYWORDS.has(word)) return "keyword";
      if (ATOMS.has(word)) return "atom";
      if (BUILTINS.has(word)) return "builtin";
      // Function definition: highlight the name after `fn`
      return "variable";
    }

    // Operators and punctuation
    if (stream.match(/^[+\-*\/%&|^~<>=!?.@]+/)) return "operator";
    if (stream.match(/^[{}()[\],;:]/)) return "punctuation";

    stream.next();
    return null;
  },

  languageData: {
    commentTokens: { line: "//", block: { open: "/*", close: "*/" } },
    indentOnInput: /^\s*[{}]$/,
    closeBrackets: { brackets: ["(", "[", "{", '"', "'", "`"] },
  },
});
