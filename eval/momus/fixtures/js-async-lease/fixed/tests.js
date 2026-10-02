const assert = require("node:assert/strict");
const {forward} = require("./forward");
const lease = {sendAsync: async message => `sent:${message}`, release() {}};
forward(lease, "ping").then(value => assert.equal(value, "sent:ping"));
