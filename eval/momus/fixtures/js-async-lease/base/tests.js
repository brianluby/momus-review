const assert = require("node:assert/strict");
assert.equal(require("./worker").handle("ping"), "sent:ping");
