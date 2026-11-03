// Preferencias de presentación que leen componentes sueltos (sin pasarlas de prop en prop).
// App las actualiza con cada snapshot.
export const ui = $state({
  /** Enseñar en la conversación las imágenes que Claude mira o que se le pegan. */
  showMedia: true,
});
