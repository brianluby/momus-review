exports.drain = queue => {
  const jobs = [];
  while (queue.length) jobs.push(queue.shift());
  return jobs;
};
