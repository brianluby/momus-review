exports.refund = (state, key, cents) => {
  if (state.refunds.has(key)) return false;
  state.refunds.add(key);
  state.balance += cents;
  return true;
};
