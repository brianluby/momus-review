exports.forward = (lease, message) => {
  try {
    return lease.sendSync(message);
  } finally {
    lease.release();
  }
};
