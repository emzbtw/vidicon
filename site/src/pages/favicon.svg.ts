// The app icon, read in place from assets/ and served as Starlight's
// default favicon path.
import icon from '../../../assets/vidicon.svg?raw';

export const GET = () => new Response(icon, { headers: { 'Content-Type': 'image/svg+xml' } });
