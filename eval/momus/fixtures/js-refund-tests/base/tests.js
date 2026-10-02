const assert = require("node:assert/strict");
const ledger = require("./ledger");
const state = {balance:100};
ledger.charge(state, 20);
assert.equal(state.balance, 80);
