import 'package:jaspr/server.dart';
import 'package:grindvakt_site/main.dart' as site;

void main() {
  Jaspr.initializeApp();
  runApp(
    Document(
      title: 'Grindvakt — Secure dynamic DNS updater',
      body: const site.GrindvaktSite(),
    ),
  );
}
