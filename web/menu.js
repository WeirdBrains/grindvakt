/* Mobile menu toggle: accessible behavior matching header markup */
(function () {
  function onReady() {
    var btn = document.getElementById('menu-toggle');
    var nav = document.getElementById('site-nav');
    if (!btn || !nav) return;
    // initial collapsed on mobile
    nav.setAttribute('data-open', 'false');
    function setOpen(open) {
      nav.setAttribute('data-open', open ? 'true' : 'false');
      btn.setAttribute('aria-expanded', open ? 'true' : 'false');
    }
    btn.addEventListener('click', function () {
      var open = nav.getAttribute('data-open') === 'true';
      setOpen(!open);
    });
    // Keyboard: Enter/Space trigger button click without double-activating
    btn.addEventListener('keydown', function (e) {
      if (e.key === 'Enter' || e.key === ' ') {
        e.preventDefault();
        btn.click();
      }
    });
    // Close on Escape and return focus to button
    document.addEventListener('keydown', function (e) {
      if (e.key === 'Escape' && nav.getAttribute('data-open') === 'true') {
        setOpen(false);
        btn.focus();
      }
    });
    // Close menu when a nav link is clicked (on mobile)
    nav.addEventListener('click', function (e) {
      var t = e.target;
      if (t && t.tagName === 'A' && nav.getAttribute('data-open') === 'true') {
        setOpen(false);
      }
    });
  }
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', onReady);
  } else {
    onReady();
  }
})();

