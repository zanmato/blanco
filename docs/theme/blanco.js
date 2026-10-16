// The docs live in docs/ below the landing page, so the title in the menu
// bar links back to it. mdBook has no setting for this.
(function () {
  const title = document.querySelector(".menu-title");
  if (!title || typeof path_to_root === "undefined") {
    return;
  }
  const link = document.createElement("a");
  link.href = path_to_root + "../";
  link.textContent = title.textContent;
  title.replaceChildren(link);
})();
