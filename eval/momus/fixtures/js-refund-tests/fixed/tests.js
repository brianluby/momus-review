const assert = require("node:assert/strict");
const ledger = require("./ledger");
const state = {balance:0, refunds:new Set()};
assert.equal(ledger.refund(state, "order-7", 30), true);
assert.equal(state.balance, 30);
assert.equal(ledger.refund(state, "order-7", 30), false);
assert.equal(state.balance, 30);
