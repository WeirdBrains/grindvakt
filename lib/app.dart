import 'package:jaspr/jaspr.dart';
import 'package:jaspr/dom.dart';

const String appVersion = '0.7.0';

class Grindvakt extends StatelessComponent {
  const Grindvakt({super.key});

  @override
  Component build(BuildContext context) {
    return div(classes: 'page', [
      // Load external CSS built with the site.
      link(href: '/styles.css', attributes: {'rel': 'stylesheet'}),
      header(classes: 'site-header', [
        a(href: '#', classes: 'brand', [
          img(
            src: '/images/grindvakt-guard-logo.jpg',
            attributes: {'alt': 'Grindvakt guard logo'},
          ),
          .text('Grindvakt'),
        ]),
        // Mobile toggle (hidden on desktop)
        Component.element(
          tag: 'button',
          classes: 'menu-toggle',
          attributes: {
            'id': 'menu-toggle',
            'aria-controls': 'site-nav',
            'aria-expanded': 'false',
          },
          children: [.text('Menu')],
        ),
        nav(id: 'site-nav', classes: 'nav', [
          a(href: '#install', [.text('Install')]),
          a(href: '#setup', [.text('Setup')]),
          a(href: '#security', [.text('Security')]),
          a(href: '#backgrounds', [.text('Backgrounds')]),
          a(href: 'https://github.com/WeirdBrains/grindvakt/releases/latest', [
            .text('Releases'),
          ]),
        ]),
      ]),
      main_([
        section(id: 'hero', classes: 'hero', [
          div(classes: 'hero-inner', [
            div(classes: 'hero-left', [
              h1([.text('Your vigilant DNS guard')]),
              p([
                .text(
                  'Grindvakt keeps your A record updated to your real public IPv4, '
                  'secured with an encrypted vault and a 24‑word recovery key. '
                  'Built in Rust. User-mode on Linux (systemd —user), macOS (launchd), '
                  'and Windows (manual-start service).',
                ),
              ]),
              div(classes: 'cta', [
                a(
                  href:
                      'https://github.com/WeirdBrains/grindvakt/releases/tag/v$appVersion',
                  classes: 'btn version',
                  [.text('v$appVersion')],
                ),
                a(
                  href: 'https://github.com/WeirdBrains/grindvakt/releases/latest',
                  classes: 'btn primary',
                  [.text('Download installers')],
                ),
                a(href: '#install', classes: 'btn', [
                  .text('Installation guide'),
                ]),
              ]),
            ]),
            div(classes: 'hero-right', [
              // Use the existing guard logo prominently as the hero ornament.
              img(
                src: '/images/grindvakt-guard-logo.jpg',
                classes: 'mjolnir',
                attributes: {'alt': '', 'aria-hidden': 'true'},
              ),
            ]),
          ]),
        ]),
        section(id: 'features', classes: 'features', [
          _feature(
            'Majority IP consensus',
            'Counts multiple sources and only accepts a strict majority. '
                'No ties. No first‑two shortcuts.',
          ),
          _feature(
            'Encrypted vault + recovery',
            'XChaCha20‑Poly1305 DEK, Argon2id password wrap, 24‑word recovery. '
                'Zero secrets on argv/env/logs.',
          ),
          _feature(
            'DNSimple integration',
            'Create/update/no‑op A records with the official SDK. Pagination and idempotency by default.',
          ),
          _feature(
            'systemd —user',
            'Oneshot service + timer, explicit enrollment. No root, no auto‑enable.',
          ),
        ]),
        section(id: 'install', classes: 'install', [
          h2([.text('Install')]),
          p([
            .text(
              'Download an installer or the portable tarball from the v$appVersion GitHub release. '
              'The macOS package and Windows MSI are unsigned. They do not start a service, '
              'load a launch agent, or enroll credentials.',
            ),
          ]),
          ul(classes: 'downloads', [
            li([
              a(
                href:
                    'https://github.com/WeirdBrains/grindvakt/releases/download/v$appVersion/grindvakt-x86_64-musl.tar.gz',
                [.text('Linux x86_64 (musl, tar.gz)')],
              ),
            ]),
            li([
              a(
                href:
                    'https://github.com/WeirdBrains/grindvakt/releases/download/v$appVersion/grindvakt_${appVersion}_amd64.deb',
                [.text('Debian/Ubuntu (.deb)')],
              ),
            ]),
            li([
              a(
                href:
                    'https://github.com/WeirdBrains/grindvakt/releases/download/v$appVersion/grindvakt-$appVersion-1.x86_64.rpm',
                [.text('Fedora/RHEL (.rpm) — unsigned (use --nosignature)')],
              ),
            ]),
            li([
              a(
                href:
                    'https://github.com/WeirdBrains/grindvakt/releases/download/v$appVersion/grindvakt-$appVersion-1-x86_64.pkg.tar.zst',
                [.text('Arch Linux (.pkg.tar.zst)')],
              ),
            ]),
            li([
              a(
                href:
                    'https://github.com/WeirdBrains/grindvakt/releases/download/v$appVersion/grindvakt-$appVersion-macos-arm64.pkg',
                [.text('macOS ARM64 (.pkg) — unsigned')],
              ),
            ]),
            li([
              a(
                href:
                    'https://github.com/WeirdBrains/grindvakt/releases/download/v$appVersion/grindvakt-$appVersion-windows-x64.msi',
                [.text('Windows x64 (.msi) — unsigned, manual-start')],
              ),
            ]),
          ]),
          p([
            .text('After the Windows MSI, run '),
            code([.text('grindvakt service install')]),
            .text(', enroll with '),
            code([.text('grindvakt service enroll-credential')]),
            .text(' (elevated), then start with '),
            code([.text('sc start Grindvakt')]),
            .text('. After the macOS package, install the LaunchAgent with '),
            code([.text('grindvakt service install')]),
            .text(' and load it later with launchctl. Linux setup is below.'),
          ]),
        ]),
        section(id: 'setup', classes: 'setup', [
          h2([.text('Setup (systemd —user)')]),
          ol([
            li([
              .text('Create a seed config (e.g., seed.toml) with your '),
              code([.text('zone')]),
              .text(', '),
              code([.text('name')]),
              .text(', '),
              code([.text('ttl')]),
              .text(', and '),
              code([.text('[schedule]')]),
              .text('. Example:'),
              Component.element(
                tag: 'pre',
                children: [
                  Component.element(
                    tag: 'code',
                    children: [
                      .text('''zone = "example.com"
name = "@"
ttl = 300

[schedule]
interval_secs = 300
'''),
                    ],
                  ),
                ],
              ),
              p([
                .text('Replace '),
                code([.text('example.com')]),
                .text(' and '),
                code([.text('name')]),
                .text(' with your actual domain and label (use '),
                code([.text('@')]),
                .text(' for apex). Then run:'),
              ]),
            ]),
            li([
              code([.text('grindvakt init --from seed.toml')]),
            ]),
            li([
              code([.text('grindvakt sync --dry-run')]),
            ]),
            li([
              code([.text('grindvakt service enroll-credential')]),
            ]),
            li([
              code([.text('grindvakt service install')]),
            ]),
            li([
              code([.text('systemctl --user enable --now grindvakt.timer')]),
            ]),
          ]),
          p([
            .text(
              'Enrollment is required before enabling the timer. Dry‑run reads your public IP and DNS state but does not write.',
            ),
          ]),
        ]),
        section(id: 'security', classes: 'security', [
          h2([.text('Security notes')]),
          ul([
            li([
              .text(
                'Strict majority for public IP; refuses ties with < 2 sources.',
              ),
            ]),
            li([.text('Vault file permissions validated; symlinks rejected.')]),
            li([
              .text(
                'Dry‑run never writes DNS. Tests cover pagination, duplicates, errors.',
              ),
            ]),
          ]),
        ]),
        section(id: 'backgrounds', classes: 'backgrounds', [
          h2([.text('Backgrounds')]),
          p([
            .text(
              'Download wallpapers featuring the Grindvakt guard logo in common desktop sizes. ',
            ),
          ]),
          div(classes: 'wallpaper-preview', [
            img(
              src: '/images/backgrounds/grindvakt-viking-preview.webp',
              attributes: {
                'alt': 'Grindvakt wallpaper preview',
                'loading': 'lazy',
              },
            ),
          ]),
          ul(classes: 'backgrounds-grid', [
            _bgCard('1920x1080'),
            _bgCard('2560x1440'),
            _bgCard('3840x2160'),
            _bgCard('1920x1200'),
            _bgCard('2560x1600'),
            _bgCard('3440x1440'),
          ]),
        ]),
      ]),
      footer(classes: 'site-footer', [
        .text('© '),
        .text('${DateTime.now().year} '),
        a(href: 'https://github.com/WeirdBrains/grindvakt', [
          .text('WeirdBrains / Grindvakt'),
        ]),
      ]),
      // External client-side script to toggle mobile menu.
      Component.element(
        tag: 'script',
        attributes: {'src': '/menu.js', 'defer': 'defer'},
      ),
    ]);
  }

  Component _feature(String title, String body) {
    return div(classes: 'feature', [
      h3([.text(title)]),
      p([.text(body)]),
    ]);
  }

  Component _bgCard(String size) {
    final href = '/images/backgrounds/grindvakt-viking-$size.png';
    final thumb = '/images/backgrounds/grindvakt-viking-$size-thumb.webp';
    final label = size == '3840x2160'
        ? '3840 × 2160 (4K, upscaled)'
        : size.replaceAll('x', ' × ');
    return li(classes: 'bg-card', [
      img(
        src: thumb,
        attributes: {
          'alt': 'Wallpaper $label',
          'loading': 'lazy',
          'width': '480',
        },
      ),
      div(classes: 'bg-caption', [
        span([.text(label)]),
        Component.element(
          tag: 'a',
          classes: 'download-link',
          attributes: {
            'href': href,
            'download': 'grindvakt-viking-$size.png',
            'aria-label': 'Download $label',
          },
          children: [.text('Download')],
        ),
      ]),
    ]);
  }
}
