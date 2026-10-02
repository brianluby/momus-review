exports.publish = async (transport, event) => transport.publish(event, {mode:"durable"});
