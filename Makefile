PREFIX ?= /usr/local
LIBDIR ?= $(PREFIX)/lib
BINDIR ?= $(PREFIX)/bin

.PHONY: all clean install

all:
	cargo build --release
	mkdir -p build
	cp -f target/release/libgtk_remote.so build/libgtk-remote.so
	cp -f target/release/libgtk_remote_gtk3.so build/libgtk-remote-gtk3.so
	cp -f target/release/libgtk_remote_gtk4.so build/libgtk-remote-gtk4.so
	cp -f target/release/gtk-remote build/gtk-remote

install: all
	install -d $(DESTDIR)$(LIBDIR) $(DESTDIR)$(BINDIR)
	install -m 755 target/release/libgtk_remote.so $(DESTDIR)$(LIBDIR)/libgtk_remote.so
	install -m 755 target/release/libgtk_remote_gtk3.so $(DESTDIR)$(LIBDIR)/libgtk_remote_gtk3.so
	install -m 755 target/release/libgtk_remote_gtk4.so $(DESTDIR)$(LIBDIR)/libgtk_remote_gtk4.so
	install -m 755 target/release/gtk-remote $(DESTDIR)$(BINDIR)/gtk-remote
	install -m 755 gtk-remote-launch $(DESTDIR)$(BINDIR)/gtk-remote-launch

clean:
	cargo clean
	rm -rf build
