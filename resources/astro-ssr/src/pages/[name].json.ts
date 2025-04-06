export const prerender = false;

export async function GET({ params }) {
  return new Response(params.name);
}
