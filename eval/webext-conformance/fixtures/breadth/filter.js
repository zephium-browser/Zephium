const probe = async path => {
  try {
    const response = await fetch(path, {cache: 'no-store'});
    return {loaded: response.ok, status: response.status};
  } catch (error) {
    return {loaded: false, error: error.message};
  }
};
return JSON.stringify({
  blocked: await probe('/analytics.analytics/analytics.min.js'),
  control: await probe('/run3-control.json'),
});
