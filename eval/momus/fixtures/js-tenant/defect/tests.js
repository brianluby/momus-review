const assert = require("node:assert/strict");
const notes = require("./notes");
const rows = [{tenant:"red", id:1, title:"old"}];
assert.equal(notes.updateTitle(rows, "red", 1, "new"), true);
assert.equal(rows[0].title, "new");
