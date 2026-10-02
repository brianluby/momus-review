const assert = require("node:assert/strict");
const notes = require("./notes");
assert.equal(notes.get([], "red", 9), undefined);
