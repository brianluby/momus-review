const assert = require("node:assert/strict");
assert.equal(require("./app").writeRecord({id:7}), '{"id":7}');
