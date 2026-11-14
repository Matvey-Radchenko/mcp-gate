// A new scoped version can 404 for a few minutes while the registry catches up.
export const notFoundWaitsMs = [15_000, 30_000, 60_000, 60_000, 60_000, 60_000];

function delay(ms) {
  return new Promise(resolve => setTimeout(resolve, ms));
}

export async function retryNotFound(run, sleep = delay) {
  for (let attempt = 0; attempt <= notFoundWaitsMs.length; attempt++) {
    try {
      return await run();
    } catch (error) {
      if (error?.notFound !== true || attempt === notFoundWaitsMs.length) throw error;
      await sleep(notFoundWaitsMs[attempt]);
    }
  }
}
