const {Lease} = require("./lease");
const {forward} = require("./forward");
exports.handle = async message => forward(new Lease(), message);
