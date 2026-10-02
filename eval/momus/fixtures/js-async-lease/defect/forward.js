exports.forward = async (lease, message) => {
  try {
    return lease.sendAsync(message);
  } finally {
    lease.release();
  }
};
