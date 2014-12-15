
# Test command for Travis
test:
	rustc src/lib.rs -O --test --cfg no_mlock
	./tars
	rm -f tars

clean:
	rm -rf doc/
	rm -rf target/
	find . \( -name '*.a' -or \
		-name '*.o' -or \
		-name '*.so' -or \
		-name 'Cargo.lock' -or \
		-name '*~' \) \
		-print -exec rm {} \;
