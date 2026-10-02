const {drain} = require("./queue");
exports.dispatch = queue => drain(queue);
