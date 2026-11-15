export async function run(h) {
  const request = async (url) => {
    const response = await fetch(url, { cache: "no-store" });
    return {
      status: response.status,
      type: response.type,
      body: await response.json(),
    };
  };
  await h.call("fetch granted host", () => request(`${h.origins.a}/data.json`));
  await h.call("fetch ungranted host", () =>
    request(`${h.origins.b}/data.json`),
  );
}
