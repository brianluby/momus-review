class Lease {
  constructor() { this.released = false; }
  sendSync(message) {
    if (this.released) throw new Error("inactive lease");
    return `sent:${message}`;
  }
  release() { this.released = true; }
}
module.exports = {Lease};
