//go:build !mocksam

// Stubs for builds without the mock SAM server. The shipped binary has no
// --mock flag at all: mockSAMRequested is a constant false, so main() always
// takes the real embedded-router path. See mock_sam.go for the tagged build.
package main

func mockSAMRequested() bool { return false }

func runMockSAM(string) {}
