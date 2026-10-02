class Lease {
  constructor() { this.released = false; }
  sendSync(message) {
    if (this.released) throw new Error("inactive lease");
    return `sent:${message}`;
  }
  async sendAsync(message) {
    await Promise.resolve();
    if (this.released) throw new Error("lease released before send completed");
    if (message === "fail") throw new Error("transport rejected send");
    return `sent:${message}`;
  }
  release() { this.released = true; }
}
module.exports = {Lease};
