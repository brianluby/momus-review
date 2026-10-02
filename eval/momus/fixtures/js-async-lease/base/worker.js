const {Lease} = require("./lease");
const {forward} = require("./forward");
exports.handle = message => forward(new Lease(), message);
