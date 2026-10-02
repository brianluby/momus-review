const {take} = require("./queue");
exports.dispatch = queue => take(queue);
