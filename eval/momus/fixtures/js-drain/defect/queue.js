exports.drain = queue => {
  const jobs = [];
  for (let index = 0; index < queue.length; index++) jobs.push(queue.shift());
  return jobs;
};
