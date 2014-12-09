# Modify mlock limits
CC=gcc
mlock: env/mlock.c
	$(CC) -o env/mlock -Wall env/mlock.c
	./env/mlock

clean:
	rm -rf doc/
	rm -rf target/
	find . \( -name '*.a' -or \
		-name '*.o' -or \
		-name '*.so' -or \
		-name 'mlock' -or \
		-name 'Cargo.lock' -or \
		-name '*~' \) \
		-print -exec rm {} \;
