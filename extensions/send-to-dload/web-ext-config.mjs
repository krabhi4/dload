// web-ext-config.mjs — Mozilla web-ext CLI configuration
// https://github.com/mozilla/web-ext#using-a-config-file
//
// Default options picked up by all `web-ext` subcommands.

export default {
  sourceDir: ".",
  noInput: true,

  build: {
    overwriteDest: true,
  },

  lint: {
    selfHosted: true,
    output: "text",
  },
};