#!/bin/bash
echo "*     hard   memlock           unlimited" >>/etc/security/limits.conf
echo "*     soft   memlock           unlimited" >>/etc/security/limits.conf
