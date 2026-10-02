const assert = require("node:assert/strict");
assert.deepEqual(require("./dispatcher").dispatch(["a"]), ["a"]);
