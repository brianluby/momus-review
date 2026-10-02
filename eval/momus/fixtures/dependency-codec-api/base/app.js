const codec = require("./vendor/codec-fixture");
exports.writeRecord = record => codec.encode(record);
