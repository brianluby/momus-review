const assert = require("node:assert/strict");
assert.equal(require("./queue").take(["a"]), "a");
