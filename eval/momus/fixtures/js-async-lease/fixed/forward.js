exports.forward = async (lease, message) => {
  try {
    return await lease.sendAsync(message);
  } finally {
    lease.release();
  }
};
