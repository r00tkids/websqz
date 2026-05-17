all: helloworld

helloworld: helloworld.c
	clang -arch arm64 -Oz \
		-fno-exceptions -fno-rtti \ # Not really needed for C, but good practice to disable in small binaries
		-ffunction-sections -fdata-sections \
		-fno-unwind-tables -fno-asynchronous-unwind-tables \
		-Wl,-dead_strip \
		-Wl,-x \
		-o helloworld helloworld.c