// Test-only preload: no real credentials or network requests. Production never loads it.
globalThis.fetch = async (input) => {
  const url = new URL(typeof input === 'string' ? input : input.url ?? input);
  if (url.origin === 'https://api.figma.com' && url.pathname === '/v1/images/fixture') {
    return Response.json({ images: { '1:1': 'https://fixture.invalid/pixel.png' } });
  }
  if (url.origin === 'https://api.figma.com' && url.pathname === '/v1/files/fixture/images') {
    return Response.json({ meta: { images: { fixture: 'https://fixture.invalid/pixel.png' } } });
  }
  if (url.href === 'https://fixture.invalid/pixel.png') {
    const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==', 'base64');
    return new Response(png, { headers: { 'content-type': 'image/png' } });
  }
  throw new Error(`Unexpected fixture request: ${url.origin}${url.pathname}`);
};
